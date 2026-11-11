//! Which Zaino or lightwalletd the node asks for compact blocks, and what that server learns.
//!
//! | `--zec-indexer` host | Kind | Connection |
//! |---|---|---|
//! | loopback or LAN | your own indexer (Zebra and Zaino) | direct, never through the proxy |
//! | `.onion` | remote indexer | through `--proxy` only; refused without one |
//! | anything else | remote indexer | through `--proxy` only; refused without one |
//!
//! There is no default server: with no `--zec-indexer` Zcash is off. A
//! server sees that blocks are downloaded (all of them: the wallet
//! trial-decrypts every output itself) and when. It never gets a key and is
//! never asked about an address or a transaction, so it cannot tell which
//! outputs are the wallet's. Over Tor it sees an exit relay, not the user's
//! address.
//!
//! `http://` is plaintext gRPC (lightwalletd's default port 9067 when none is
//! given); `https://` is gRPC over TLS (port 443 by default).

use serde_json::{json, Value};

use crate::ZecNetwork;

const DEFAULT_HTTP_PORT: u16 = 9067;
const DEFAULT_HTTPS_PORT: u16 = 443;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerKind {
    /// Loopback or LAN: the user's own indexer.
    Own,
    /// A `.onion` indexer.
    Onion,
    /// An indexer on the public internet.
    Public,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// `http(s)://host:port`, no path.
    pub url: String,
    /// Lowercase, without brackets.
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub kind: ServerKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerConfig {
    pub network: ZecNetwork,
    pub servers: Vec<Server>,
    /// `socks5h://HOST:PORT` for remote servers.
    pub proxy: Option<String>,
}

impl IndexerConfig {
    /// `urls` from `--zec-indexer`, tried in order. `proxy` is the node's `--proxy`.
    pub fn new(network: ZecNetwork, urls: &[String], proxy: Option<String>) -> Result<Self, String> {
        if urls.is_empty() {
            return Err("--zec-indexer needs a URL".into());
        }
        let mut servers = Vec::new();
        for url in urls {
            let server = parse_url(url)?;
            match server.kind {
                ServerKind::Onion if proxy.is_none() => {
                    return Err(format!("{} is an onion service; start the node with --proxy socks5h://127.0.0.1:9050", server.url));
                }
                ServerKind::Public if proxy.is_none() => {
                    return Err(format!(
                        "{} is a remote Zcash indexer; it is only reached through Tor. Add --proxy socks5h://127.0.0.1:9050, or run Zebra and Zaino yourself",
                        server.url
                    ));
                }
                _ => {}
            }
            servers.push(server);
        }
        Ok(Self { network, servers, proxy })
    }

    /// Whether connections to `server` go through the proxy.
    pub fn proxied(&self, server: &Server) -> bool {
        server.kind != ServerKind::Own && self.proxy.is_some()
    }

    /// `own-node`, `remote-tor`, or `public` (a remote server reached over clearnet).
    pub fn privacy(&self) -> &'static str {
        if self.servers.iter().any(|server| server.kind != ServerKind::Own && !self.proxied(server)) {
            "public"
        } else if self.servers.iter().any(|server| server.kind != ServerKind::Own) {
            "remote-tor"
        } else {
            "own-node"
        }
    }

    pub fn reveals(&self) -> &'static str {
        match self.privacy() {
            "own-node" => "Your own indexer serves every block. No one else learns anything.",
            "remote-tor" => "The indexer sees a Tor exit, not your address, and that blocks are being downloaded, all of them. It never gets a key and is never asked about an address or a transaction, so it cannot tell which payments are yours.",
            _ => "The indexer sees your IP address and that blocks are being downloaded, all of them. It never gets a key, so it cannot tell which payments are yours. Start the node with --proxy to use Tor.",
        }
    }
}

/// What `/network` says about the Zcash backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendStatus {
    pub config: Option<IndexerConfig>,
    pub network: ZecNetwork,
    pub tip: Option<u64>,
    /// The server currently asked, if one has answered.
    pub active: Option<String>,
    /// What the server says it runs, from its last `GetLightdInfo`.
    pub software: Option<String>,
    pub error: Option<String>,
    /// Set by a test double instead of a configuration.
    pub mock: bool,
}

impl BackendStatus {
    pub fn off(network: ZecNetwork) -> Self {
        Self { config: None, network, tip: None, active: None, software: None, error: None, mock: false }
    }

    pub fn value(&self) -> Value {
        if self.mock {
            return json!({
                "backend": "mock",
                "network": self.network.as_str(),
                "privacy": "own-node",
                "reveals": "A test chain in memory.",
                "servers": [],
                "active": null,
                "software": null,
                "proxied": false,
                "tip": self.tip,
                "error": self.error,
            });
        }
        let Some(config) = &self.config else {
            return json!({
                "backend": "none",
                "network": self.network.as_str(),
                "privacy": "none",
                "reveals": "Zcash is off. Start the node with --zec-indexer URL (your own Zebra and Zaino, or a remote indexer over --proxy) to accept Zcash.",
                "servers": [],
                "active": null,
                "software": null,
                "proxied": false,
                "tip": null,
                "error": null,
            });
        };
        let own = config.servers.iter().all(|server| server.kind == ServerKind::Own);
        json!({
            "backend": if own { "own-node" } else { "remote-indexer" },
            "network": self.network.as_str(),
            "privacy": config.privacy(),
            "reveals": config.reveals(),
            "servers": config.servers.iter().map(|server| server.url.as_str()).collect::<Vec<_>>(),
            "active": self.active,
            "software": self.software,
            "proxied": config.servers.iter().any(|server| config.proxied(server)),
            "tip": self.tip,
            "error": self.error,
        })
    }
}

/// `http(s)://host[:port]` with nothing after it.
fn parse_url(text: &str) -> Result<Server, String> {
    let text = text.trim().trim_end_matches('/');
    let refused = || format!("--zec-indexer must be http://HOST:PORT or https://HOST:PORT, not {text:?}");
    let (scheme, authority) = text.split_once("://").ok_or_else(refused)?;
    if !matches!(scheme, "http" | "https") || authority.is_empty() || authority.contains(['/', '@', '?', '#', ' ']) {
        return Err(refused());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(refused)?;
        (host.to_string(), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host.to_string(), Some(port)),
            Some(_) => return Err(refused()),
            None => (authority.to_string(), None),
        }
    };
    if host.is_empty() {
        return Err(refused());
    }
    let tls = scheme == "https";
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(refused)?,
        None if tls => DEFAULT_HTTPS_PORT,
        None => DEFAULT_HTTP_PORT,
    };
    let host = host.to_ascii_lowercase();
    let shown = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    Ok(Server { url: format!("{scheme}://{shown}:{port}"), kind: classify(&host), host, port, tls })
}

fn classify(host: &str) -> ServerKind {
    if host.ends_with(".onion") {
        return ServerKind::Onion;
    }
    if host == "localhost" || host.ends_with(".local") {
        return ServerKind::Own;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => ServerKind::Own,
        Ok(std::net::IpAddr::V6(ip)) if ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 => ServerKind::Own,
        _ => ServerKind::Public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOR: Option<&str> = Some("socks5h://127.0.0.1:9050");

    fn config(urls: &[&str], proxy: Option<&str>) -> Result<IndexerConfig, String> {
        let urls: Vec<String> = urls.iter().map(|url| url.to_string()).collect();
        IndexerConfig::new(ZecNetwork::Testnet, &urls, proxy.map(String::from))
    }

    #[test]
    fn own_indexers_are_direct_and_get_default_ports() {
        let own = config(&["http://127.0.0.1", "https://192.168.1.5:8137/", "http://[::1]:9067", "http://zaino.local:8137"], TOR).unwrap();
        let urls: Vec<&str> = own.servers.iter().map(|server| server.url.as_str()).collect();
        assert_eq!(urls, ["http://127.0.0.1:9067", "https://192.168.1.5:8137", "http://[::1]:9067", "http://zaino.local:8137"]);
        assert!(own.servers.iter().all(|server| server.kind == ServerKind::Own && !own.proxied(server)));
        assert_eq!((own.servers[0].tls, own.servers[1].tls), (false, true));
        assert_eq!(own.privacy(), "own-node");
        assert!(config(&["http://127.0.0.1"], None).is_ok(), "an own indexer needs no proxy");
        assert_eq!(config(&["https://zec.example.org"], TOR).unwrap().servers[0].url, "https://zec.example.org:443");
    }

    #[test]
    fn remote_indexers_are_reached_only_through_tor() {
        let err = config(&["https://zec.example.org:443"], None).unwrap_err();
        assert!(err.contains("only reached through Tor"), "{err}");
        let err = config(&[&format!("http://{}.onion:9067", "a".repeat(56))], None).unwrap_err();
        assert!(err.contains("onion service"), "{err}");
        let tor = config(&["https://zec.example.org", "http://127.0.0.1:8137"], TOR).unwrap();
        assert!(tor.proxied(&tor.servers[0]) && !tor.proxied(&tor.servers[1]));
        assert_eq!(tor.privacy(), "remote-tor");
        assert!(tor.reveals().contains("Tor exit"));
        assert_eq!(tor.servers[0].kind, ServerKind::Public);
    }

    #[test]
    fn malformed_urls_are_refused() {
        for bad in ["zec.example.org", "grpc://127.0.0.1:9067", "http://127.0.0.1:9067/path", "http://user@127.0.0.1", "http://:9067", "http://127.0.0.1:0", "http://127.0.0.1:99999", "http://[::1"] {
            assert!(config(&[bad], TOR).is_err(), "{bad}");
        }
        assert!(config(&[], TOR).is_err());
    }

    #[test]
    fn status_says_what_the_server_sees() {
        let off = BackendStatus::off(ZecNetwork::Mainnet).value();
        assert_eq!((&off["backend"], &off["privacy"], &off["network"]), (&json!("none"), &json!("none"), &json!("mainnet")));
        assert!(off["reveals"].as_str().unwrap().contains("--zec-indexer"));
        let config = config(&["https://zec.example.org"], TOR).unwrap();
        let status = BackendStatus { config: Some(config), tip: Some(7), ..BackendStatus::off(ZecNetwork::Testnet) };
        let value = status.value();
        assert_eq!((&value["backend"], &value["privacy"], &value["proxied"]), (&json!("remote-indexer"), &json!("remote-tor"), &json!(true)));
        assert_eq!((&value["servers"][0], &value["tip"]), (&json!("https://zec.example.org:443"), &json!(7)));
    }
}
