//! How the node's own outside requests leave the machine.
//!
//! Exchange rates (Coinbase) and chain data (public Esplora) are the only
//! requests the node makes to servers it does not run. Without a proxy they
//! go over clearnet and show the user's IP address to those servers. With
//! `--proxy socks5h://127.0.0.1:9050` (Tor; 9150 for Tor Browser) both go
//! through SOCKS5, and host names are resolved by the proxy, not locally.
//! `--no-rates` stops the rates request altogether. Escrow mail to loopback
//! peers never uses the proxy.

use std::time::Duration;

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Egress {
    /// `socks5h://HOST:PORT`, or `None` for direct connections.
    pub proxy: Option<String>,
    pub live_rates: bool,
}

impl Default for Egress {
    fn default() -> Self {
        Self {
            proxy: None,
            live_rates: true,
        }
    }
}

impl Egress {
    /// Accept `socks5://` or `socks5h://HOST:PORT`. Either way the proxy resolves names.
    pub fn parse_proxy(text: &str) -> Result<String, String> {
        let text = text.trim().trim_end_matches('/');
        let refused = || format!("--proxy must be socks5h://HOST:PORT (Tor is socks5h://127.0.0.1:9050), not {text:?}");
        let authority = text
            .strip_prefix("socks5h://")
            .or_else(|| text.strip_prefix("socks5://"))
            .ok_or_else(refused)?;
        let (host, port) = authority.rsplit_once(':').ok_or_else(refused)?;
        if host.is_empty() || host.contains(['/', '@', '?', '#']) || port.parse::<u16>().map_or(true, |port| port == 0) {
            return Err(refused());
        }
        Ok(format!("socks5h://{authority}"))
    }

    /// A ureq agent with these timeouts that goes through the proxy, if any.
    pub(crate) fn ureq_agent(&self, connect: Duration, read: Duration) -> Result<ureq::Agent, String> {
        let mut builder = ureq::AgentBuilder::new().timeout_connect(connect).timeout_read(read).redirects(0);
        if let Some(proxy) = &self.proxy {
            // ureq sends host names to a socks5:// proxy unresolved.
            let address = proxy.replacen("socks5h://", "socks5://", 1);
            builder = builder.proxy(ureq::Proxy::new(address).map_err(|err| err.to_string())?);
        }
        Ok(builder.build())
    }

    pub(crate) fn reqwest_proxy(&self) -> Result<Option<reqwest::Proxy>, String> {
        self.proxy
            .as_deref()
            .map(|proxy| reqwest::Proxy::all(proxy).map_err(|err| err.to_string()))
            .transpose()
    }

    /// The `/network` fields about outside requests.
    pub(crate) fn value(&self, public_chain_server: bool) -> Value {
        let mut clearnet = Vec::new();
        if self.proxy.is_none() {
            if public_chain_server {
                clearnet.push("the chain server");
            }
            if self.live_rates {
                clearnet.push("the exchange-rate server (Coinbase)");
            }
        }
        let warning = (!clearnet.is_empty()).then(|| {
            format!(
                "This node reaches {} over clearnet, so they see your IP address. Start the node with --proxy socks5h://127.0.0.1:9050 to use Tor{}.",
                clearnet.join(" and "),
                if self.live_rates { ", or --no-rates to skip prices" } else { "" },
            )
        });
        json!({
            "proxy": self.proxy,
            "liveRates": self.live_rates,
            "clearnet": !clearnet.is_empty(),
            "privacyWarning": warning,
        })
    }
}

#[cfg(test)]
pub(crate) mod fake_socks {
    //! A SOCKS5 server that records the destination of one CONNECT and hangs up.

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    /// The proxy URL and a receiver for `(address type, host)` of each CONNECT.
    pub(crate) fn start() -> (String, mpsc::Receiver<(u8, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("socks5h://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut greeting = [0u8; 2];
                if stream.read_exact(&mut greeting).is_err() {
                    continue;
                }
                let mut methods = vec![0u8; greeting[1] as usize];
                let _ = stream.read_exact(&mut methods);
                let _ = stream.write_all(&[5, 0]);
                let mut request = [0u8; 4];
                if stream.read_exact(&mut request).is_err() {
                    continue;
                }
                let host = match request[3] {
                    3 => {
                        let mut len = [0u8; 1];
                        let _ = stream.read_exact(&mut len);
                        let mut name = vec![0u8; len[0] as usize];
                        let _ = stream.read_exact(&mut name);
                        String::from_utf8_lossy(&name).into_owned()
                    }
                    1 => {
                        let mut ip = [0u8; 4];
                        let _ = stream.read_exact(&mut ip);
                        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
                    }
                    other => format!("type {other}"),
                };
                let _ = tx.send((request[3], host));
                // Host unreachable: the client gives up without a request.
                let _ = stream.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]);
            }
        });
        (url, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies_are_socks5_with_remote_names() {
        assert_eq!(Egress::parse_proxy("socks5://127.0.0.1:9050").unwrap(), "socks5h://127.0.0.1:9050");
        assert_eq!(Egress::parse_proxy("socks5h://localhost:9150/").unwrap(), "socks5h://localhost:9150");
        for bad in ["http://127.0.0.1:8080", "socks5://127.0.0.1", "socks5://:9050", "socks5://u:p@h:1", "127.0.0.1:9050", "socks5://h:0"] {
            assert!(Egress::parse_proxy(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn clearnet_is_flagged_until_a_proxy_is_set() {
        let direct = Egress::default().value(true);
        assert_eq!(direct["clearnet"], true);
        assert!(direct["privacyWarning"].as_str().unwrap().contains("IP address"));
        let no_rates = Egress { proxy: None, live_rates: false }.value(false);
        assert_eq!(no_rates["clearnet"], false);
        assert!(no_rates["privacyWarning"].is_null());
        let tor = Egress { proxy: Some("socks5h://127.0.0.1:9050".into()), live_rates: true }.value(true);
        assert_eq!(tor["clearnet"], false);
        assert_eq!(tor["proxy"], "socks5h://127.0.0.1:9050");
    }
}
