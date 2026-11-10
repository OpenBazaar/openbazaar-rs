//! How the node's own outside requests leave the machine.
//!
//! Exchange rates (Coinbase, Kraken) and chain data (filter peers, or Esplora)
//! are the requests the node makes to machines it does not run. Without a
//! proxy those requests are not made, so a public server does not see this
//! machine's IP. With `--proxy socks5h://127.0.0.1:9050` (Tor; 9150 for Tor
//! Browser) they go through SOCKS5, and host names are resolved by the proxy.
//! `--no-rates` stops the rates request even when a proxy is set. A peer or
//! server on this machine or the LAN is still reached directly. Escrow mail
//! to loopback peers never uses the proxy.

use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicU64, Ordering};
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

    /// A proxy that sends a different SOCKS username on every connection.
    /// Tor puts streams with different credentials on different circuits, so
    /// no exit or server sees two chain queries arrive from the same address.
    /// The client must not keep connections alive, or reused ones share a circuit.
    pub(crate) fn reqwest_proxy(&self) -> Result<Option<reqwest::Proxy>, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let Some(proxy) = &self.proxy else { return Ok(None) };
        let authority = proxy.trim_start_matches("socks5h://").to_string();
        reqwest::Url::parse(proxy).map_err(|err| err.to_string())?;
        let session = std::process::id();
        Ok(Some(reqwest::Proxy::custom(move |_| {
            let stream = NEXT.fetch_add(1, Ordering::Relaxed);
            Some(format!("socks5h://ob-{session}-{stream}:x@{authority}"))
        })))
    }

    /// Whether `proxy` (`socks5h://HOST:PORT`) accepts a SOCKS greeting.
    /// A greeting is not a circuit and does not send a wallet address.
    pub(crate) fn proxy_reachable(proxy: &str) -> bool {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::time::Duration;
        let authority = proxy.trim_start_matches("socks5h://").trim_start_matches("socks5://");
        let Ok(mut addresses) = authority.to_socket_addrs() else {
            return false;
        };
        let Some(address) = addresses.next() else {
            return false;
        };
        let timeout = Duration::from_millis(400);
        let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
            return false;
        };
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        if stream.write_all(&[5, 1, 0]).is_err() {
            return false;
        }
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).is_ok() && reply[0] == 5
    }

    /// The `/network` fields about outside requests.
    pub(crate) fn value(&self, public_chain_server: bool) -> Value {
        let skipped = self.proxy.is_none() && (public_chain_server || self.live_rates);
        let warning = skipped.then(|| {
            "Rates and public chain servers are not contacted without --proxy, so they do not see this machine's IP. Pass --proxy socks5h://127.0.0.1:9050 to fetch them over Tor (9150 is Tor Browser's port). HTTP_PROXY, HTTPS_PROXY, and ALL_PROXY are ignored."
                .to_string()
        });
        json!({
            "proxy": self.proxy,
            "liveRates": self.live_rates,
            "clearnet": false,
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
                if methods.contains(&2) {
                    // Username and password, as Tor accepts for circuit isolation.
                    let _ = stream.write_all(&[5, 2]);
                    let mut version = [0u8; 2];
                    let _ = stream.read_exact(&mut version);
                    let _ = stream.read_exact(&mut vec![0u8; version[1] as usize]);
                    let mut len = [0u8; 1];
                    let _ = stream.read_exact(&mut len);
                    let _ = stream.read_exact(&mut vec![0u8; len[0] as usize]);
                    let _ = stream.write_all(&[1, 0]);
                } else {
                    let _ = stream.write_all(&[5, 0]);
                }
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
        assert_eq!(direct["clearnet"], false);
        assert!(direct["privacyWarning"].as_str().unwrap().contains("not contacted"));
        let no_rates = Egress { proxy: None, live_rates: false }.value(false);
        assert_eq!(no_rates["clearnet"], false);
        assert!(no_rates["privacyWarning"].is_null());
        let tor = Egress { proxy: Some("socks5h://127.0.0.1:9050".into()), live_rates: true }.value(true);
        assert_eq!(tor["clearnet"], false);
        assert_eq!(tor["proxy"], "socks5h://127.0.0.1:9050");
    }

    #[test]
    fn a_proxy_answers_when_it_accepts_a_socks_greeting() {
        let (url, _seen) = fake_socks::start();
        assert!(Egress::proxy_reachable(&url));
        assert!(!Egress::proxy_reachable("socks5h://127.0.0.1:1"));
    }
}
