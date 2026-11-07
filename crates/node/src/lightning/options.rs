//! `ob-node serve` flags for Lightning, and the defaults per network.
//!
//! | flag | meaning |
//! |---|---|
//! | `--lightning` | run the LDK node (off by default) |
//! | `--ln-network signet\|testnet4\|regtest` | test mode only; default `signet` (Mutinynet) |
//! | `--ln-chain esplora:URL\|electrum:URL\|bitcoind:USER:PASS@HOST:PORT` | the LDK node's chain view |
//! | `--lsp NODE_ID@HOST:PORT` / `--lsp-token T` | the LSP for inbound liquidity |
//! | `--no-lsp` | no LSP: open channels by hand |
//! | `--lsp-max-fee-sats N` | the least the fee cap for a just-in-time channel may be (default 10000) |
//! | `--ln-rgs URL` / `--ln-p2p-gossip` | where the channel graph comes from |
//! | `--ln-listen HOST:PORT` | accept inbound peer connections (default: none) |
//!
//! Test mode runs Lightning on Mutinynet, a public signet with 30-second
//! blocks, because no public LSP serves testnet4 (checked November 2026).
//! The default LSP there is the public "C=" LSPS2 node, and the chain view
//! and gossip come from mutinynet.com. Mainnet has no defaults: it needs
//! `--ln-chain` and either `--lsp` or `--no-lsp`. OpenBazaar runs no LSP.

use std::path::Path;

use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::{ChainSource, GossipSource, LnConfig, LspConfig, PeerAddress};

use crate::network::NetworkMode;

/// The public Mutinynet LSPS2 node and the token its operators publish for testing.
pub(crate) const MUTINYNET_LSP: &str = "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b@44.219.111.31:39735";
const MUTINYNET_LSP_TOKEN: &str = "JZWN9YLW";
const MUTINYNET_ESPLORA: &str = "https://mutinynet.com/api";
const MUTINYNET_RGS: &str = "https://rgs.mutinynet.com/snapshot";
const TESTNET4_ESPLORA: &str = "https://mempool.space/testnet4/api";
/// 5 % of the payment, but never a cap below `--lsp-max-fee-sats`.
const MAX_LSP_FEE_BPS: u64 = 500;
const MIN_LSP_FEE_CAP_SATS: u64 = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LnFlags {
    pub on: bool,
    network: Option<String>,
    chain: Option<String>,
    lsp: Option<String>,
    lsp_token: Option<String>,
    no_lsp: bool,
    rgs: Option<String>,
    p2p_gossip: bool,
    listen: Option<String>,
    max_fee_sats: Option<u64>,
}

impl LnFlags {
    /// Take `flag` if it is a Lightning flag. `None` when it is not one.
    pub fn take(&mut self, flag: &str, mut value: impl FnMut(&str) -> Result<String, String>) -> Option<Result<(), String>> {
        let mut text = |slot: &mut Option<String>| value(flag).map(|text| *slot = Some(text));
        Some(match flag {
            "--lightning" => {
                self.on = true;
                Ok(())
            }
            "--ln-network" => text(&mut self.network),
            "--ln-chain" => text(&mut self.chain),
            "--lsp" => text(&mut self.lsp),
            "--lsp-token" => text(&mut self.lsp_token),
            "--no-lsp" => {
                self.no_lsp = true;
                Ok(())
            }
            "--ln-rgs" => text(&mut self.rgs),
            "--ln-p2p-gossip" => {
                self.p2p_gossip = true;
                Ok(())
            }
            "--ln-listen" => text(&mut self.listen),
            "--lsp-max-fee-sats" => {
                let mut raw = None;
                match text(&mut raw) {
                    Ok(()) => match raw.unwrap_or_default().parse::<u64>() {
                        Ok(sats) => {
                            self.max_fee_sats = Some(sats);
                            Ok(())
                        }
                        Err(_) => Err("--lsp-max-fee-sats must be a number".into()),
                    },
                    Err(err) => Err(err),
                }
            }
            _ => return None,
        })
    }

    /// The LDK configuration these flags select on `mode`, or `None` while
    /// Lightning is off. `storage_dir` is filled in by the node.
    pub fn plan(&self, mode: NetworkMode) -> Result<Option<LnConfig>, String> {
        let named = self.network.is_some()
            || self.chain.is_some()
            || self.lsp.is_some()
            || self.lsp_token.is_some()
            || self.no_lsp
            || self.rgs.is_some()
            || self.p2p_gossip
            || self.listen.is_some()
            || self.max_fee_sats.is_some();
        if !self.on {
            return if named { Err("Lightning flags need --lightning".into()) } else { Ok(None) };
        }
        if self.lsp.is_some() && self.no_lsp {
            return Err("Choose --lsp or --no-lsp, not both".into());
        }
        if self.rgs.is_some() && self.p2p_gossip {
            return Err("Choose --ln-rgs or --ln-p2p-gossip, not both".into());
        }
        let network = match (mode, self.network.as_deref()) {
            (NetworkMode::Mainnet, None) => Network::Bitcoin,
            (NetworkMode::Mainnet, Some(_)) => return Err("--ln-network is for test mode; mainnet Lightning is bitcoin".into()),
            (NetworkMode::Testnet4, None | Some("signet") | Some("mutinynet")) => Network::Signet,
            (NetworkMode::Testnet4, Some("testnet4")) => Network::Testnet4,
            (NetworkMode::Testnet4, Some("regtest")) => Network::Regtest,
            (_, Some(other)) => return Err(format!("--ln-network must be signet, testnet4, or regtest, not {other:?}")),
        };
        let mutinynet = network == Network::Signet;
        let chain = match (&self.chain, network) {
            (Some(text), _) => ChainSource::parse(text)?,
            (None, Network::Signet) => ChainSource::Esplora(MUTINYNET_ESPLORA.into()),
            (None, Network::Testnet4) => ChainSource::Esplora(TESTNET4_ESPLORA.into()),
            (None, _) => return Err("Lightning on this network needs --ln-chain (your own node, or an Esplora/Electrum server you choose)".into()),
        };
        let lsp = match (&self.lsp, self.no_lsp) {
            (Some(text), _) => Some(text.parse::<PeerAddress>()?),
            (None, true) => None,
            (None, false) if mutinynet => Some(MUTINYNET_LSP.parse::<PeerAddress>()?),
            (None, false) => {
                return Err("Lightning on this network has no default LSP. Name one with --lsp NODE_ID@HOST:PORT, or --no-lsp to open channels by hand".into())
            }
        };
        let token = match (&self.lsp_token, &self.lsp) {
            (Some(token), _) => Some(token.clone()),
            (None, None) if mutinynet && lsp.is_some() => Some(MUTINYNET_LSP_TOKEN.into()),
            _ => None,
        };
        if token.is_some() && lsp.is_none() {
            return Err("--lsp-token needs an LSP".into());
        }
        let gossip = match (&self.rgs, self.p2p_gossip) {
            (Some(url), _) => GossipSource::Rgs(url.clone()),
            (None, true) => GossipSource::P2p,
            (None, false) if mutinynet => GossipSource::Rgs(MUTINYNET_RGS.into()),
            (None, false) => GossipSource::P2p,
        };
        if let Some(listen) = &self.listen {
            if listen.rsplit_once(':').and_then(|(_, port)| port.parse::<u16>().ok()).is_none() {
                return Err("--ln-listen needs HOST:PORT".into());
            }
        }
        Ok(Some(LnConfig {
            network,
            storage_dir: Path::new("").to_path_buf(),
            chain,
            gossip,
            lsp: lsp.map(|peer| LspConfig { peer, token }),
            listen: self.listen.clone(),
            max_lsp_fee_bps: MAX_LSP_FEE_BPS,
            min_lsp_fee_cap_sats: self.max_fee_sats.unwrap_or(MIN_LSP_FEE_CAP_SATS),
        }))
    }
}

/// How `GET /network` and the UI name a Lightning network.
pub(crate) fn network_name(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "bitcoin",
        Network::Signet => "signet",
        Network::Testnet4 => "testnet4",
        Network::Regtest => "regtest",
        _ => "testnet",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(args: &[&str]) -> Result<LnFlags, String> {
        let mut flags = LnFlags::default();
        let mut iter = args.iter();
        while let Some(flag) = iter.next() {
            flags.take(flag, |_| iter.next().map(|s| s.to_string()).ok_or_else(|| "needs a value".to_string())).expect("a Lightning flag")?;
        }
        Ok(flags)
    }

    #[test]
    fn off_by_default_and_test_mode_defaults_to_mutinynet_with_its_lsp() {
        assert_eq!(LnFlags::default().plan(NetworkMode::Testnet4).unwrap().map(|c| c.network), None);
        assert!(flags(&["--no-lsp"]).unwrap().plan(NetworkMode::Testnet4).is_err(), "Lightning flags without --lightning");
        let plan = flags(&["--lightning"]).unwrap().plan(NetworkMode::Testnet4).unwrap().unwrap();
        assert_eq!(plan.network, Network::Signet);
        assert_eq!(plan.chain, ChainSource::Esplora(MUTINYNET_ESPLORA.into()));
        let lsp = plan.lsp.unwrap();
        assert_eq!(lsp.peer.to_string(), MUTINYNET_LSP);
        assert_eq!(lsp.token.as_deref(), Some(MUTINYNET_LSP_TOKEN));
        assert_eq!(plan.min_lsp_fee_cap_sats, MIN_LSP_FEE_CAP_SATS);
        let manual = flags(&["--lightning", "--no-lsp", "--ln-p2p-gossip"]).unwrap().plan(NetworkMode::Testnet4).unwrap().unwrap();
        assert!(manual.lsp.is_none());
        assert_eq!(manual.gossip, GossipSource::P2p);
    }

    #[test]
    fn testnet4_and_mainnet_need_an_explicit_lsp_choice() {
        let testnet4 = flags(&["--lightning", "--ln-network", "testnet4"]).unwrap();
        assert!(testnet4.plan(NetworkMode::Testnet4).unwrap_err().contains("no default LSP"));
        let main = flags(&["--lightning"]).unwrap();
        assert!(main.plan(NetworkMode::Mainnet).unwrap_err().contains("--ln-chain"));
        let main = flags(&["--lightning", "--ln-chain", "electrum:ssl://electrum.example.org:50002"]).unwrap();
        assert!(main.plan(NetworkMode::Mainnet).unwrap_err().contains("no default LSP"));
        let id = "031b301307574bbe9b9ac7b79cbe1700e31e544513eae0b5d7497483083f99e581";
        let main = flags(&["--lightning", "--ln-chain", "bitcoind:u:p@127.0.0.1:8332", "--lsp", &format!("{id}@45.79.192.236:9735")]).unwrap();
        let plan = main.plan(NetworkMode::Mainnet).unwrap().unwrap();
        assert_eq!(plan.network, Network::Bitcoin);
        assert!(plan.lsp.unwrap().token.is_none(), "the Mutinynet token never goes to another LSP");
        assert!(flags(&["--lightning", "--ln-network", "signet"]).unwrap().plan(NetworkMode::Mainnet).is_err());
        assert!(flags(&["--lightning", "--lsp", "x", "--no-lsp"]).unwrap().plan(NetworkMode::Testnet4).is_err());
        assert!(flags(&["--lightning", "--lsp-max-fee-sats", "lots"]).is_err());
    }
}
