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

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

/// Build a stream-isolated SOCKS5h proxy URL with fresh random credentials:
/// `socks5h://ob-<random_hex>:<random_hex>@<host>:<port>`.
///
/// Tor's `IsolateSOCKSAuth` assigns each unique username/password pair its own
/// circuit, so separate address queries cannot be correlated at the exit or
/// Esplora server. Rejects anything that is not a valid `socks5://` or
/// `socks5h://HOST:PORT` proxy (including clearnet/http URLs).
pub(crate) fn isolated_socks5_proxy_url(proxy: &str) -> Result<String, String> {
    let normalized = Egress::parse_proxy(proxy)?;
    let authority = normalized
        .strip_prefix("socks5h://")
        .ok_or_else(|| format!("proxy {proxy:?} is not socks5h"))?;
    let mut rng = StdRng::from_os_rng();
    let mut user_bytes = [0u8; 8];
    let mut pass_bytes = [0u8; 8];
    rng.fill_bytes(&mut user_bytes);
    rng.fill_bytes(&mut pass_bytes);
    let user = crate::escrow_mail::hex(&user_bytes);
    let pass = crate::escrow_mail::hex(&pass_bytes);
    Ok(format!("socks5h://ob-{user}:{pass}@{authority}"))
}

/// `socks5://` or `socks5h://HOST:PORT` as a socket address.
///
/// A numeric host, and the name `localhost`, are parsed here. `to_socket_addrs`
/// asks the OS resolver, and on Windows that call can sit for minutes even for
/// `127.0.0.1` while the network stack is busy. The desktop app's built-in Tor
/// proxy is always a loopback address, and startup used to die in that lookup:
/// the shell killed the node before it listened, so the window never opened.
pub(crate) fn proxy_addr(proxy: &str) -> Result<SocketAddr, String> {
    let authority = proxy
        .trim_start_matches("socks5h://")
        .trim_start_matches("socks5://");
    // Refuse an empty host before numeric_proxy and before any resolver call.
    // On Windows getaddrinfo answers an empty name with every interface address
    // on this machine, so `socks5h://:9050` would resolve to the node itself.
    let Some((host, _port)) = authority.rsplit_once(':') else {
        return Err(format!("proxy {authority} has no port"));
    };
    if host.trim().is_empty() {
        return Err(format!("proxy {authority} has no host"));
    }
    if let Some(addr) = numeric_proxy(authority) {
        return Ok(addr);
    }
    authority
        .to_socket_addrs()
        .map_err(|err| format!("proxy {authority}: {err}"))?
        .next()
        .ok_or_else(|| format!("proxy {authority} has no address"))
}

/// An address that does not need the OS resolver: `127.0.0.1:9050`,
/// `[::1]:9050`, or `localhost:9050`.
fn numeric_proxy(authority: &str) -> Option<SocketAddr> {
    if let Ok(addr) = authority.parse::<SocketAddr>() {
        return Some(addr);
    }
    let (host, port) = authority.rsplit_once(':')?;
    let port = port.parse::<u16>().ok().filter(|port| *port != 0)?;
    host.eq_ignore_ascii_case("localhost")
        .then(|| SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

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
        if host.trim().is_empty() || host.contains(['/', '@', '?', '#']) || port.parse::<u16>().map_or(true, |port| port == 0) {
            return Err(refused());
        }
        Ok(format!("socks5h://{authority}"))
    }

    /// A ureq agent with these timeouts that goes through the proxy, if any.
    /// All requests through this agent share the same SOCKS credentials and
    /// therefore the same Tor circuit. Use [`Self::ureq_agent_isolated`] when
    /// successive requests should use different circuits.
    pub(crate) fn ureq_agent(&self, connect: Duration, read: Duration) -> Result<ureq::Agent, String> {
        let mut builder = ureq::AgentBuilder::new().timeout_connect(connect).timeout_read(read).redirects(0);
        if let Some(proxy) = &self.proxy {
            // ureq sends host names to a socks5:// proxy unresolved.
            let address = proxy.replacen("socks5h://", "socks5://", 1);
            builder = builder.proxy(ureq::Proxy::new(address).map_err(|err| err.to_string())?);
        }
        Ok(builder.build())
    }

    /// Like [`Self::ureq_agent`] but with a unique SOCKS username so Tor
    /// puts this request on its own circuit. Use for rate and price fetches
    /// that should not share a circuit with prior requests.
    pub(crate) fn ureq_agent_isolated(&self, connect: Duration, read: Duration) -> Result<ureq::Agent, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut builder = ureq::AgentBuilder::new().timeout_connect(connect).timeout_read(read).redirects(0);
        if let Some(proxy) = &self.proxy {
            let authority = proxy.trim_start_matches("socks5h://");
            let session = std::process::id();
            let stream = NEXT.fetch_add(1, Ordering::Relaxed);
            let address = format!("socks5://ob-rate-{session}-{stream}:x@{authority}");
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
        let Ok(address) = proxy_addr(proxy) else {
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;

    /// SOCKS5 CONNECT: greeting, optional username, destination. Leaves the port consumed.
    fn connect(stream: &mut std::net::TcpStream) -> Option<(u8, String)> {
        let mut greeting = [0u8; 2];
        stream.read_exact(&mut greeting).ok()?;
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
        stream.read_exact(&mut request).ok()?;
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
        let _ = stream.read_exact(&mut [0u8; 2]);
        Some((request[3], host))
    }

    /// The proxy URL and a receiver for `(address type, host)` of each CONNECT.
    pub(crate) fn start() -> (String, mpsc::Receiver<(u8, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("socks5h://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Some(seen) = connect(&mut stream) else { continue };
                let _ = tx.send(seen);
                // Host unreachable: the client gives up without a request.
                let _ = stream.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]);
            }
        });
        (url, rx)
    }

    /// A proxy whose every destination answers one HTTP request with `answer`.
    /// Returns the proxy URL and how many requests were answered.
    pub(crate) fn start_http(answer: &'static str) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("socks5h://{}", listener.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                if connect(&mut stream).is_none() {
                    continue;
                }
                let _ = stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
                if stream.read(&mut [0u8; 2048]).is_ok_and(|read| read > 0) {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let _ = stream.write_all(answer.as_bytes());
                }
            }
        });
        (url, asked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies_are_socks5_with_remote_names() {
        assert_eq!(Egress::parse_proxy("socks5://127.0.0.1:9050").unwrap(), "socks5h://127.0.0.1:9050");
        assert_eq!(Egress::parse_proxy("socks5h://localhost:9150/").unwrap(), "socks5h://localhost:9150");
        for bad in ["http://127.0.0.1:8080", "socks5://127.0.0.1", "socks5://:9050", "socks5:// :9050", "socks5://u:p@h:1", "127.0.0.1:9050", "socks5://h:0"] {
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
    fn a_loopback_proxy_is_parsed_without_the_resolver() {
        let started = std::time::Instant::now();
        let v4 = proxy_addr("socks5h://127.0.0.1:9050").unwrap();
        assert_eq!(v4, "127.0.0.1:9050".parse().unwrap());
        let local = proxy_addr("socks5h://localhost:9150").unwrap();
        assert_eq!(local, "127.0.0.1:9150".parse().unwrap());
        let v6 = proxy_addr("socks5://[::1]:9050").unwrap();
        assert_eq!(v6, "[::1]:9050".parse().unwrap());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "a numeric proxy went through the OS resolver"
        );
        assert!(proxy_addr("socks5h://:9050").is_err());
        assert!(proxy_addr("socks5h:// :9050").is_err(), "a whitespace host is not a host");
    }

    #[test]
    fn a_proxy_answers_when_it_accepts_a_socks_greeting() {
        let (url, _seen) = fake_socks::start();
        assert!(Egress::proxy_reachable(&url));
        assert!(!Egress::proxy_reachable("socks5h://127.0.0.1:1"));
    }

    #[test]
    fn isolated_socks5_proxy_url_generates_unique_credentials_and_rejects_clearnet() {
        let first = isolated_socks5_proxy_url("socks5h://127.0.0.1:9050").unwrap();
        let second = isolated_socks5_proxy_url("socks5h://127.0.0.1:9050").unwrap();
        assert_ne!(first, second, "each call must get unique Tor circuit credentials");

        for url in [&first, &second] {
            let parsed = url::Url::parse(url).expect("valid URL");
            assert_eq!(parsed.scheme(), "socks5h");
            assert_eq!(parsed.host_str(), Some("127.0.0.1"));
            assert_eq!(parsed.port(), Some(9050));
            assert!(parsed.username().starts_with("ob-"), "username: {}", parsed.username());
            assert_eq!(parsed.username().len(), 3 + 16);
            let pass = parsed.password().expect("password present");
            assert_eq!(pass.len(), 16);
        }

        // Plain socks5:// is normalized to socks5h:// and preserves host:port.
        let v5 = isolated_socks5_proxy_url("socks5://localhost:9150/").unwrap();
        let parsed_v5 = url::Url::parse(&v5).unwrap();
        assert_eq!(parsed_v5.scheme(), "socks5h");
        assert_eq!(parsed_v5.host_str(), Some("localhost"));
        assert_eq!(parsed_v5.port(), Some(9150));

        // Clearnet / non-socks5 / malformed URLs are rejected.
        for bad in [
            "",
            "http://127.0.0.1:8080",
            "https://mempool.space",
            "127.0.0.1:9050",
            "socks5://127.0.0.1",
            "socks5://u:p@127.0.0.1:9050",
            "socks5h://:9050",
        ] {
            assert!(isolated_socks5_proxy_url(bad).is_err(), "should reject {bad:?}");
        }
    }
}
