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
//! A public Lightning peer, Esplora chain source, or Rapid Gossip Sync server
//! is reached through `--proxy` (socks5h), so it does not see this machine's
//! IP. Without `--proxy` those stay refused, and only a chain source, LSP,
//! and listen address on this machine or the LAN are allowed. Electrum,
//! bitcoind, and `--ln-listen` stay direct, so a public one is refused.
//! Testnet4 and regtest have no default LSP: name one with `--lsp`, or
//! `--no-lsp`. OpenBazaar runs no LSP.
//!
//! Mainnet Lightning requires `--proxy`. There is no mainnet LSP default:
//! the seller names one with `--lsp`, or passes `--no-lsp`.

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
    pub fn plan(&self, mode: NetworkMode, proxy: Option<&str>) -> Result<Option<LnConfig>, String> {
        let named = self.network.is_some()
            || self.chain.is_some()
            || self.lsp.is_some()
            || self.lsp_token.is_some()
            || self.no_lsp
            || self.rgs.is_some()
            || self.p2p_gossip
            || self.listen.is_some()
            || self.max_fee_sats.is_some();
        if mode == NetworkMode::Mainnet && proxy.is_none() && (self.on || named) {
            return Err(super::MAINNET_BLOCKED.into());
        }
        if !self.on {
            return if named { Err("Lightning flags need --lightning".into()) } else { Ok(None) };
        }
        if self.lsp.is_some() && self.no_lsp {
            return Err("Choose --lsp or --no-lsp, not both".into());
        }
        if self.rgs.is_some() && self.p2p_gossip {
            return Err("Choose --ln-rgs or --ln-p2p-gossip, not both".into());
        }
        let network = if mode == NetworkMode::Mainnet {
            match self.network.as_deref() {
                None | Some("bitcoin") | Some("mainnet") => Network::Bitcoin,
                Some(other) => return Err(format!("--ln-network on mainnet must be bitcoin, not {other:?}")),
            }
        } else {
            match self.network.as_deref() {
                None | Some("signet") | Some("mutinynet") => Network::Signet,
                Some("testnet4") => Network::Testnet4,
                Some("regtest") => Network::Regtest,
                Some("bitcoin") | Some("mainnet") => {
                    return Err("Mainnet Lightning is selected with --network mainnet and --proxy. There is no default LSP".into())
                }
                Some(other) => return Err(format!("--ln-network must be signet, testnet4, or regtest, not {other:?}")),
            }
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
        let config = LnConfig {
            network,
            storage_dir: Path::new("").to_path_buf(),
            chain,
            gossip,
            lsp: lsp.map(|peer| LspConfig { peer, token }),
            listen: self.listen.clone(),
            max_lsp_fee_bps: MAX_LSP_FEE_BPS,
            min_lsp_fee_cap_sats: self.max_fee_sats.unwrap_or(MIN_LSP_FEE_CAP_SATS),
            proxy: proxy.map(str::to_string),
        };
        if let Some(peer) = config.dials_in_the_clear() {
            return Err(format!(
                "Lightning would connect to {peer} in the clear, and that host would see this machine's IP. Pass --proxy socks5h://127.0.0.1:9050 so a public peer, Esplora, and gossip go through Tor, or point them at this machine or the LAN. Electrum, bitcoind, and --ln-listen stay direct"
            ));
        }
        Ok(Some(config))
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
    fn off_by_default_and_a_public_signet_peer_is_refused() {
        assert_eq!(LnFlags::default().plan(NetworkMode::Testnet4, None).unwrap().map(|c| c.network), None);
        assert!(flags(&["--no-lsp"]).unwrap().plan(NetworkMode::Testnet4, None).is_err(), "Lightning flags without --lightning");
        let public = flags(&["--lightning"]).unwrap().plan(NetworkMode::Testnet4, None).unwrap_err();
        assert!(public.contains("in the clear"), "{public}");
        assert!(public.contains("mutinynet.com") || public.contains("44.219.111.31"), "{public}");
        let local = flags(&["--lightning", "--ln-chain", "bitcoind:u:p@127.0.0.1:18443", "--no-lsp", "--ln-p2p-gossip"])
            .unwrap()
            .plan(NetworkMode::Testnet4, None)
            .unwrap()
            .unwrap();
        assert_eq!(local.network, Network::Signet);
        assert!(local.chain.is_local());
        assert!(local.lsp.is_none());
        assert_eq!(local.gossip, GossipSource::P2p);
        assert!(local.clearnet_peer().is_none());
    }

    #[test]
    fn testnet4_and_regtest_need_an_explicit_lsp_choice() {
        let testnet4 = flags(&["--lightning", "--ln-network", "testnet4"]).unwrap();
        assert!(testnet4.plan(NetworkMode::Testnet4, None).unwrap_err().contains("no default LSP"));
        let id = "031b301307574bbe9b9ac7b79cbe1700e31e544513eae0b5d7497483083f99e581";
        let lsp = format!("{id}@45.79.192.236:9735");
        let public = flags(&["--lightning", "--ln-network", "testnet4", "--lsp", &lsp]).unwrap();
        assert!(public.plan(NetworkMode::Testnet4, None).unwrap_err().contains("in the clear"));
        let local_lsp = format!("{id}@127.0.0.1:9735");
        let testnet4 = flags(&[
            "--lightning",
            "--ln-network",
            "testnet4",
            "--ln-chain",
            "bitcoind:u:p@127.0.0.1:18443",
            "--lsp",
            &local_lsp,
            "--ln-p2p-gossip",
        ])
        .unwrap();
        let plan = testnet4.plan(NetworkMode::Testnet4, None).unwrap().unwrap();
        assert_eq!(plan.network, Network::Testnet4);
        assert!(plan.lsp.unwrap().token.is_none(), "the Mutinynet token never goes to another LSP");
        let regtest = flags(&["--lightning", "--ln-network", "regtest"]).unwrap();
        assert!(regtest.plan(NetworkMode::Testnet4, None).unwrap_err().contains("--ln-chain"));
        let regtest = flags(&["--lightning", "--ln-network", "regtest", "--ln-chain", "bitcoind:u:p@127.0.0.1:18443", "--no-lsp"]).unwrap();
        assert_eq!(regtest.plan(NetworkMode::Testnet4, None).unwrap().unwrap().network, Network::Regtest);
        assert!(flags(&["--lightning", "--lsp", "x", "--no-lsp"]).unwrap().plan(NetworkMode::Testnet4, None).is_err());
        assert!(flags(&["--lightning", "--lsp-max-fee-sats", "lots"]).is_err());
    }

    #[test]
    fn mainnet_refuses_every_lightning_flag() {
        let id = "031b301307574bbe9b9ac7b79cbe1700e31e544513eae0b5d7497483083f99e581";
        let lsp = format!("{id}@45.79.192.236:9735");
        let cases: [&[&str]; 9] = [
            &["--lightning"],
            &["--lightning", "--ln-chain", "bitcoind:u:p@127.0.0.1:8332", "--lsp", &lsp],
            &["--lightning", "--ln-chain", "electrum:ssl://electrum.example.org:50002", "--no-lsp"],
            &["--lightning", "--ln-network", "signet"],
            &["--no-lsp"],
            &["--lsp", &lsp, "--lsp-token", "T"],
            &["--ln-rgs", "https://rgs.example.org"],
            &["--ln-listen", "0.0.0.0:9735"],
            &["--lsp-max-fee-sats", "5000"],
        ];
        for args in cases {
            let err = flags(args).unwrap().plan(NetworkMode::Mainnet, None).unwrap_err();
            assert_eq!(err, super::super::MAINNET_BLOCKED, "{args:?}");
        }
        assert!(matches!(LnFlags::default().plan(NetworkMode::Mainnet, None), Ok(None)), "no flags: Lightning is just off");

        let serve = |args: &[&str]| crate::network::ServeOptions::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).map(|_| ());
        assert_eq!(serve(&["--network", "mainnet", "--lightning", "--no-lsp"]).unwrap_err(), super::super::MAINNET_BLOCKED);
        assert_eq!(serve(&["--network", "mainnet", "--lsp", &lsp]).unwrap_err(), super::super::MAINNET_BLOCKED);
        assert!(serve(&["--network", "testnet4", "--lightning"]).unwrap_err().contains("in the clear"));
        assert!(serve(&[
            "--network",
            "testnet4",
            "--lightning",
            "--ln-chain",
            "bitcoind:u:p@127.0.0.1:18443",
            "--no-lsp",
            "--ln-p2p-gossip",
        ])
        .is_ok());
        assert!(serve(&["--network", "mainnet"]).is_ok());
    }

    #[test]
    fn a_public_peer_goes_through_the_proxy_and_mainnet_has_no_default_lsp() {
        let proxy = "socks5h://127.0.0.1:9050";
        let id = "031b301307574bbe9b9ac7b79cbe1700e31e544513eae0b5d7497483083f99e581";
        let lsp = format!("{id}@203.0.113.9:9735");

        let signet = flags(&["--lightning"]).unwrap().plan(NetworkMode::Testnet4, Some(proxy)).unwrap().unwrap();
        assert!(signet.dials_in_the_clear().is_none(), "Mutinynet is carried by the proxy");
        assert!(signet.clearnet_peer().is_some(), "the public peer is still named");
        assert_eq!(signet.proxy.as_deref(), Some(proxy));

        let direct = flags(&["--lightning", "--ln-chain", "electrum:electrum.example.org:50001", "--no-lsp", "--ln-p2p-gossip"])
            .unwrap()
            .plan(NetworkMode::Testnet4, Some(proxy))
            .unwrap_err();
        assert!(direct.contains("in the clear"), "{direct}");
        let listen = flags(&[
            "--lightning",
            "--ln-chain",
            "bitcoind:u:p@127.0.0.1:18443",
            "--no-lsp",
            "--ln-p2p-gossip",
            "--ln-listen",
            "1.2.3.4:9735",
        ])
        .unwrap()
        .plan(NetworkMode::Testnet4, Some(proxy))
        .unwrap_err();
        assert!(listen.contains("in the clear"), "{listen}");

        let needs_chain = flags(&["--lightning"]).unwrap().plan(NetworkMode::Mainnet, Some(proxy)).unwrap_err();
        assert!(needs_chain.contains("--ln-chain"), "{needs_chain}");
        let needs_lsp = flags(&["--lightning", "--ln-chain", "esplora:https://mempool.space/api"])
            .unwrap()
            .plan(NetworkMode::Mainnet, Some(proxy))
            .unwrap_err();
        assert!(needs_lsp.contains("no default LSP"), "{needs_lsp}");
        let mainnet = flags(&[
            "--lightning",
            "--ln-chain",
            "esplora:https://mempool.space/api",
            "--lsp",
            &lsp,
            "--ln-p2p-gossip",
        ])
        .unwrap()
        .plan(NetworkMode::Mainnet, Some(proxy))
        .unwrap()
        .unwrap();
        assert_eq!(mainnet.network, Network::Bitcoin);
        assert!(mainnet.dials_in_the_clear().is_none());
        assert!(mainnet.lsp.unwrap().token.is_none(), "mainnet does not borrow the Mutinynet token");

        let serve = |args: &[&str]| crate::network::ServeOptions::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>()).map(|_| ());
        assert!(serve(&[
            "--network",
            "mainnet",
            "--proxy",
            proxy,
            "--lightning",
            "--ln-chain",
            "esplora:https://mempool.space/api",
            "--lsp",
            &lsp,
            "--ln-p2p-gossip",
        ])
        .is_ok());
        assert_eq!(
            serve(&[
                "--network",
                "mainnet",
                "--lightning",
                "--ln-chain",
                "esplora:https://mempool.space/api",
                "--lsp",
                &lsp,
                "--ln-p2p-gossip",
            ])
            .unwrap_err(),
            super::super::MAINNET_BLOCKED
        );
        assert!(serve(&["--network", "testnet4", "--proxy", proxy, "--lightning"]).is_ok());
        assert!(serve(&["--network", "testnet4", "--lightning"]).unwrap_err().contains("in the clear"));
    }
}
