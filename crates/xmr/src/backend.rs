//! Which monerod the node talks to, and what that daemon learns.
//!
//! | `--xmr-daemon` host | Kind | Connection |
//! |---|---|---|
//! | loopback or LAN | your own node | direct, never through the proxy |
//! | `.onion` | remote node | through `--proxy` only; refused without one |
//! | anything else | remote node | through `--proxy` only; refused without one |
//!
//! There is no default node and no light-wallet server: with no
//! `--xmr-daemon` Monero is off. A remote node sees which blocks are
//! downloaded (all of them), the ring members fetched when paying, and the
//! transaction it is asked to broadcast. It never gets a key, so it cannot
//! tell which outputs are the wallet's. Over Tor it sees an exit relay, not
//! the user's address.

use serde_json::{json, Value};

use crate::XmrNetwork;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// Loopback or LAN: the user's own monerod.
    Own,
    /// A `.onion` daemon.
    Onion,
    /// A daemon on the public internet.
    Public,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonNode {
    /// `http(s)://host:port`, no path.
    pub url: String,
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    pub network: XmrNetwork,
    pub nodes: Vec<DaemonNode>,
    /// `socks5h://HOST:PORT` for remote nodes.
    pub proxy: Option<String>,
}

impl DaemonConfig {
    /// `urls` from `--xmr-daemon`, tried in order. `proxy` is the node's `--proxy`.
    pub fn new(network: XmrNetwork, urls: &[String], proxy: Option<String>) -> Result<Self, String> {
        if urls.is_empty() {
            return Err("--xmr-daemon needs a URL".into());
        }
        let mut nodes = Vec::new();
        for url in urls {
            let (url, host) = parse_url(url, network)?;
            let kind = classify(&host);
            match kind {
                NodeKind::Onion if proxy.is_none() => {
                    return Err(format!("{url} is an onion service; start the node with --proxy socks5h://127.0.0.1:9050"));
                }
                NodeKind::Public if proxy.is_none() => {
                    return Err(format!(
                        "{url} is a remote Monero node; it is only reached through Tor. Add --proxy socks5h://127.0.0.1:9050, or use your own monerod"
                    ));
                }
                _ => {}
            }
            nodes.push(DaemonNode { url, kind });
        }
        Ok(Self { network, nodes, proxy })
    }

    /// Whether requests to `node` go through the proxy.
    pub fn proxied(&self, node: &DaemonNode) -> bool {
        node.kind != NodeKind::Own && self.proxy.is_some()
    }

    /// `own-node`, `remote-tor`, or `public` (a remote node reached over clearnet).
    pub fn privacy(&self) -> &'static str {
        if self.nodes.iter().any(|node| node.kind != NodeKind::Own && !self.proxied(node)) {
            "public"
        } else if self.nodes.iter().any(|node| node.kind != NodeKind::Own) {
            "remote-tor"
        } else {
            "own-node"
        }
    }

    pub fn reveals(&self) -> &'static str {
        match self.privacy() {
            "own-node" => "Your own monerod serves every block and broadcasts your payments. No one else learns anything.",
            "remote-tor" => "The remote node sees a Tor exit, not your address. It sees every block being downloaded, the decoy rings fetched when you pay, and the transactions you broadcast. It never gets a key, so it cannot tell which outputs are yours.",
            _ => "The remote node sees your IP address, every block being downloaded, the decoy rings fetched when you pay, and the transactions you broadcast. It never gets a key, so it cannot tell which outputs are yours. Start the node with --proxy to use Tor.",
        }
    }
}

/// What `/network` says about the Monero backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendStatus {
    pub config: Option<DaemonConfig>,
    pub network: XmrNetwork,
    pub tip: Option<u64>,
    /// The node currently asked, if one has answered.
    pub active: Option<String>,
    pub error: Option<String>,
}

impl BackendStatus {
    pub fn off(network: XmrNetwork) -> Self {
        Self { config: None, network, tip: None, active: None, error: None }
    }

    pub fn value(&self) -> Value {
        let Some(config) = &self.config else {
            return json!({
                "backend": "none",
                "network": self.network.as_str(),
                "privacy": "none",
                "reveals": "Monero is off. Start the node with --xmr-daemon URL (your own monerod, or a remote node over --proxy) to accept or pay in Monero.",
                "nodes": [],
                "proxied": false,
                "tip": null,
                "error": null,
            });
        };
        let own = config.nodes.iter().all(|node| node.kind == NodeKind::Own);
        json!({
            "backend": if own { "own-node" } else { "remote-node" },
            "network": self.network.as_str(),
            "privacy": config.privacy(),
            "reveals": config.reveals(),
            "nodes": config.nodes.iter().map(|node| node.url.as_str()).collect::<Vec<_>>(),
            "active": self.active,
            "proxied": config.nodes.iter().any(|node| config.proxied(node)),
            "tip": self.tip,
            "error": self.error,
        })
    }
}

/// `http(s)://host[:port]` with nothing after it. Returns the URL and the host.
fn parse_url(text: &str, network: XmrNetwork) -> Result<(String, String), String> {
    let text = text.trim().trim_end_matches('/');
    let refused = || format!("--xmr-daemon must be http://HOST:PORT or https://HOST:PORT, not {text:?}");
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
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(refused)?,
        None => network.default_rpc_port(),
    };
    let shown = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    Ok((format!("{scheme}://{shown}:{port}"), host.to_ascii_lowercase()))
}

fn classify(host: &str) -> NodeKind {
    if host.ends_with(".onion") {
        return NodeKind::Onion;
    }
    if host == "localhost" || host.ends_with(".local") {
        return NodeKind::Own;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) if ip.is_loopback() || ip.is_private() || ip.is_link_local() => NodeKind::Own,
        Ok(std::net::IpAddr::V6(ip)) if ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 => NodeKind::Own,
        _ => NodeKind::Public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOR: Option<&str> = Some("socks5h://127.0.0.1:9050");

    fn config(network: XmrNetwork, urls: &[&str], proxy: Option<&str>) -> Result<DaemonConfig, String> {
        let urls: Vec<String> = urls.iter().map(|url| url.to_string()).collect();
        DaemonConfig::new(network, &urls, proxy.map(String::from))
    }

    #[test]
    fn own_nodes_are_direct_and_get_the_default_port() {
        let own = config(XmrNetwork::Stagenet, &["http://127.0.0.1", "http://192.168.1.5:18089/"], TOR).unwrap();
        assert_eq!(own.nodes[0].url, "http://127.0.0.1:38081");
        assert_eq!(own.nodes[1].url, "http://192.168.1.5:18089");
        assert!(own.nodes.iter().all(|node| node.kind == NodeKind::Own && !own.proxied(node)));
        assert_eq!(own.privacy(), "own-node");
        assert_eq!(BackendStatus { config: Some(own), ..BackendStatus::off(XmrNetwork::Stagenet) }.value()["backend"], "own-node");
    }

    #[test]
    fn remote_nodes_need_tor_on_mainnet_and_onions_always() {
        assert!(config(XmrNetwork::Mainnet, &["http://node.example.org:18089"], None).unwrap_err().contains("--proxy"));
        assert!(config(XmrNetwork::Stagenet, &["http://node.example.org:38081"], None).unwrap_err().contains("--proxy"));
        assert!(config(XmrNetwork::Stagenet, &["http://abc.onion:38081"], None).unwrap_err().contains("--proxy"));
        let tor = config(XmrNetwork::Mainnet, &["http://node.example.org:18089", "http://abc.onion"], TOR).unwrap();
        assert_eq!(tor.privacy(), "remote-tor");
        assert!(tor.nodes.iter().all(|node| tor.proxied(node)));
        let stagenet = config(XmrNetwork::Stagenet, &["http://node.example.org:38081"], TOR).unwrap();
        assert_eq!(stagenet.privacy(), "remote-tor");
    }

    #[test]
    fn urls_with_paths_users_or_other_schemes_are_refused() {
        for bad in [
            "node.example.org:18081",
            "socks5://127.0.0.1:1",
            "http://user:pass@127.0.0.1:18081",
            "http://127.0.0.1:18081/json_rpc",
            "http://127.0.0.1:0",
            "http://127.0.0.1:18081?x=1",
            "http://",
        ] {
            assert!(config(XmrNetwork::Stagenet, &[bad], TOR).is_err(), "{bad}");
        }
        assert!(config(XmrNetwork::Stagenet, &[], TOR).is_err());
    }

    #[test]
    fn off_says_how_to_turn_it_on() {
        let off = BackendStatus::off(XmrNetwork::Mainnet).value();
        assert_eq!(off["backend"], "none");
        assert!(off["reveals"].as_str().unwrap().contains("--xmr-daemon"));
    }
}
