//! Which Lightning network, chain view, gossip, and LSP an LDK node uses.
//!
//! Nothing here picks a default; the node crate does, per network, and says
//! so in `docs/lightning.md`.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use ldk_node::bitcoin::secp256k1::PublicKey;
use ldk_node::bitcoin::Network;

/// `NODE_ID@HOST:PORT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerAddress {
    pub node_id: PublicKey,
    /// `HOST:PORT`; the host may be an IP address or a DNS name.
    pub address: String,
}

impl FromStr for PeerAddress {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let (id, address) = text.trim().split_once('@').ok_or("Give the peer as NODE_ID@HOST:PORT.")?;
        let node_id = PublicKey::from_str(id).map_err(|_| "The node id is not a 33-byte public key in hex.".to_string())?;
        let (host, port) = address.rsplit_once(':').ok_or("Give the peer as NODE_ID@HOST:PORT.")?;
        if host.is_empty() || port.parse::<u16>().map_or(true, |port| port == 0) {
            return Err("Give the peer as NODE_ID@HOST:PORT.".into());
        }
        Ok(Self { node_id, address: address.to_string() })
    }
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.node_id, self.address)
    }
}

/// A Lightning Service Provider that sells inbound liquidity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspConfig {
    pub peer: PeerAddress,
    /// Some LSPs hand out access tokens; most public ones need none.
    pub token: Option<String>,
}

/// Where the LDK node learns about the chain. LDK node has no compact-filter
/// client, so this is a different source from the on-chain wallet's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainSource {
    Esplora(String),
    Electrum(String),
    BitcoindRpc { host: String, port: u16, user: String, password: String },
}

impl ChainSource {
    /// `esplora:URL`, `electrum:URL`, or `bitcoind:USER:PASS@HOST:PORT`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || "Give the Lightning chain source as esplora:URL, electrum:URL, or bitcoind:USER:PASS@HOST:PORT.".to_string();
        let (kind, rest) = text.trim().split_once(':').ok_or_else(bad)?;
        match kind {
            "esplora" if rest.starts_with("http://") || rest.starts_with("https://") => Ok(Self::Esplora(rest.to_string())),
            "electrum" if !rest.is_empty() => Ok(Self::Electrum(rest.to_string())),
            "bitcoind" => {
                let (auth, addr) = rest.rsplit_once('@').ok_or_else(bad)?;
                let (user, password) = auth.split_once(':').ok_or_else(bad)?;
                let (host, port) = addr.rsplit_once(':').ok_or_else(bad)?;
                let port = port.parse().map_err(|_| bad())?;
                Ok(Self::BitcoindRpc { host: host.to_string(), port, user: user.to_string(), password: password.to_string() })
            }
            _ => Err(bad()),
        }
    }

    /// What `GET /network` says about this source, without credentials.
    pub fn describe(&self) -> String {
        match self {
            Self::Esplora(url) => format!("esplora {url}"),
            Self::Electrum(url) => format!("electrum {url}"),
            Self::BitcoindRpc { host, port, .. } => format!("bitcoind {host}:{port}"),
        }
    }

    /// True when the source is on this machine or the LAN.
    pub fn is_local(&self) -> bool {
        let host = match self {
            Self::Esplora(url) | Self::Electrum(url) => url
                .split("://")
                .last()
                .unwrap_or(url)
                .split(['/', ':'])
                .next()
                .unwrap_or_default()
                .to_string(),
            Self::BitcoindRpc { host, .. } => host.clone(),
        };
        host == "localhost"
            || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
                std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
                std::net::IpAddr::V6(v6) => v6.is_loopback(),
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GossipSource {
    /// Learn the channel graph from connected peers (the LSP). No third party.
    P2p,
    /// Rapid Gossip Sync snapshots from this URL. Faster; the server sees the node's IP.
    Rgs(String),
}

#[derive(Clone, Debug)]
pub struct LnConfig {
    pub network: Network,
    /// LDK's files (sealed) live here.
    pub storage_dir: PathBuf,
    pub chain: ChainSource,
    pub gossip: GossipSource,
    /// `None`: no LSP. Receiving then needs a channel opened by hand.
    pub lsp: Option<LspConfig>,
    /// Accept inbound peer connections here. `None`: outbound only.
    pub listen: Option<String>,
    /// The most an LSP may take for a just-in-time channel, as a share of the
    /// payment in basis points (1/100 of a percent), with a floor in sats.
    pub max_lsp_fee_bps: u64,
    pub min_lsp_fee_cap_sats: u64,
}

impl LnConfig {
    /// The LSP fee cap for a payment of `amount_msat`.
    pub fn lsp_fee_cap_msat(&self, amount_msat: u64) -> u64 {
        let share = amount_msat.saturating_mul(self.max_lsp_fee_bps) / 10_000;
        share.max(self.min_lsp_fee_cap_sats.saturating_mul(1_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b";

    #[test]
    fn parses_peers_and_chain_sources() {
        let peer: PeerAddress = format!("{ID}@44.219.111.31:39735").parse().unwrap();
        assert_eq!(peer.address, "44.219.111.31:39735");
        assert_eq!(peer.to_string(), format!("{ID}@44.219.111.31:39735"));
        assert!(format!("{ID}@lsp.example.org:9735").parse::<PeerAddress>().is_ok());
        for bad in ["", ID, "02aa@1.2.3.4:9735", &format!("{ID}@1.2.3.4"), &format!("{ID}@1.2.3.4:0")] {
            assert!(bad.parse::<PeerAddress>().is_err(), "{bad}");
        }
        assert_eq!(ChainSource::parse("esplora:https://mutinynet.com/api").unwrap(), ChainSource::Esplora("https://mutinynet.com/api".into()));
        let rpc = ChainSource::parse("bitcoind:ob:secret@127.0.0.1:18443").unwrap();
        assert!(rpc.is_local());
        assert!(!rpc.describe().contains("secret"));
        assert!(!ChainSource::parse("esplora:https://mutinynet.com/api").unwrap().is_local());
        assert!(ChainSource::parse("esplora:http://192.168.1.4:3002").unwrap().is_local());
        assert!(ChainSource::parse("esplora:ftp://x").is_err());
        assert!(ChainSource::parse("bitcoind:127.0.0.1:18443").is_err());
    }

    #[test]
    fn the_lsp_fee_cap_has_a_floor() {
        let config = LnConfig {
            network: Network::Regtest,
            storage_dir: PathBuf::new(),
            chain: ChainSource::Esplora("http://127.0.0.1:3002".into()),
            gossip: GossipSource::P2p,
            lsp: None,
            listen: None,
            max_lsp_fee_bps: 500,
            min_lsp_fee_cap_sats: 5_000,
        };
        assert_eq!(config.lsp_fee_cap_msat(1_000_000_000), 50_000_000);
        assert_eq!(config.lsp_fee_cap_msat(10_000_000), 5_000_000);
    }
}
