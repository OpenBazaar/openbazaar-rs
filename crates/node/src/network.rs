//! Which Bitcoin network this node runs on, and the checks that keep them apart.
//!
//! Testnet4 is the default. Mainnet is only ever chosen by the exact flag
//! `--network mainnet`; nothing else selects it. Each network has its own
//! data directory, so keys, wallets, address book, orders, and mailbox never
//! share a file.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use bitcoin::{Address, Amount, Network};
use openbazaar_pay::{Bolt12Offer, SilentPaymentCode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkMode {
    Testnet4,
    Mainnet,
}

impl NetworkMode {
    /// Parse the value of `--network`. Only `testnet4` and `mainnet` are accepted.
    pub fn from_flag(value: &str) -> Result<Self, String> {
        match value {
            "testnet4" => Ok(NetworkMode::Testnet4),
            "mainnet" => Ok(NetworkMode::Mainnet),
            other => Err(format!(
                "--network must be testnet4 or mainnet, not {other:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NetworkMode::Testnet4 => "testnet4",
            NetworkMode::Mainnet => "mainnet",
        }
    }

    pub fn bitcoin(self) -> Network {
        match self {
            NetworkMode::Testnet4 => Network::Testnet4,
            NetworkMode::Mainnet => Network::Bitcoin,
        }
    }

    /// BIP 44 coin type: 0 on mainnet, 1 on every test network.
    pub(crate) fn coin_type(self) -> u32 {
        match self {
            NetworkMode::Testnet4 => 1,
            NetworkMode::Mainnet => 0,
        }
    }

    pub fn data_dir(self, root: &Path) -> PathBuf {
        root.join(self.as_str())
    }

    /// Public Esplora servers for this network, used only with
    /// `--public-fallback`. Every one serves only this network; the chain
    /// client also checks its genesis block.
    pub fn default_esplora(self) -> &'static [&'static str] {
        match self {
            NetworkMode::Testnet4 => &["https://mempool.space/testnet4/api", "https://blockstream.info/testnet4/api"],
            NetworkMode::Mainnet => &["https://mempool.space/api"],
        }
    }

    /// The Monero network paid on this network. Monero has no testnet4, so
    /// testnet4 pays on stagenet; mainnet pays on mainnet.
    pub fn monero(self) -> openbazaar_xmr::XmrNetwork {
        match self {
            NetworkMode::Testnet4 => openbazaar_xmr::XmrNetwork::Stagenet,
            NetworkMode::Mainnet => openbazaar_xmr::XmrNetwork::Mainnet,
        }
    }

    pub fn default_confirmations(self) -> u32 {
        match self {
            NetworkMode::Testnet4 => 1,
            NetworkMode::Mainnet => 3,
        }
    }
}

impl fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where `--vault` keeps the vault key. The passphrase itself is read at start, not from argv.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultChoice {
    Keystore,
    Passphrase,
    File,
}

/// Everything `serve` needs to know before it opens the data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    pub port: u16,
    pub network: NetworkMode,
    pub data_root: PathBuf,
    /// `--esplora`: use exactly these servers, tried in order, instead of
    /// compact filters. May be repeated or comma-separated. Empty by default.
    pub esplora_urls: Vec<String>,
    /// `--peer HOST:PORT`: the user's own node, the only filter peer.
    pub peers: Vec<(String, u16)>,
    /// `--public-fallback` / `--no-public-fallback`: ask the public Esplora
    /// servers while no filter peer answers. On by default on testnet4 only.
    pub public_fallback: bool,
    pub confirmations: u32,
    pub poll_secs: u64,
    /// `--escrow-vendor SELLER=URL`: the node that holds that seller's escrow keys.
    pub escrow_vendors: Vec<(String, String)>,
    /// `--moderator URL`: the node that runs the testnet4 fixture moderator.
    pub moderator_url: Option<String>,
    /// `--moderate`: answer dispute letters as the testnet4 fixture moderator.
    pub moderate: bool,
    /// `--vault keystore|passphrase|file`. Keystore unless named.
    pub vault: VaultChoice,
    /// `--shop-origin URL`: the browser shop allowed to call the node.
    pub shop_origin: String,
    /// False with `--no-pair-codes` (the desktop app pairs its own window).
    pub pair_codes: bool,
    /// `--proxy socks5h://HOST:PORT` for rates and chain queries.
    pub proxy: Option<String>,
    /// False with `--no-rates`.
    pub live_rates: bool,
    /// `--xmr-daemon URL`: monerod RPC for Monero direct pay. Empty turns Monero off.
    pub xmr_daemons: Vec<String>,
    /// `--lightning` and its flags; see `lightning/options.rs`.
    pub lightning: crate::LnFlags,
    /// `--follow URL`: copy the store another node sells there. May be repeated.
    pub follows: Vec<String>,
    /// `--local-shop DIR`: serve the web export on the loopback listener.
    pub local_shop: Option<PathBuf>,
}

impl ServeOptions {
    /// Parse the arguments after `serve`.
    ///
    /// With no `--network`, the node runs on testnet4. Mainnet is never a fallback.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut port = 8787u16;
        let mut network = None;
        let mut data_root = PathBuf::from("data");
        let mut esplora: Vec<String> = Vec::new();
        let mut peers: Vec<String> = Vec::new();
        let mut public_fallback = None;
        let mut confirmations = None;
        let mut poll_secs = 30u64;
        let mut escrow_vendors = Vec::new();
        let mut moderator_url = None;
        let mut moderate = false;
        let mut vault = VaultChoice::Keystore;
        let mut shop_origin = crate::http_guard::DEFAULT_SHOP_ORIGIN.to_string();
        let mut pair_codes = true;
        let mut proxy = None;
        let mut live_rates = true;
        let mut xmr_daemons: Vec<String> = Vec::new();
        let mut lightning = crate::LnFlags::default();
        let mut follows: Vec<String> = Vec::new();
        let mut local_shop = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| {
                iter.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            if let Some(taken) = lightning.take(arg, &mut value) {
                taken?;
                continue;
            }
            match arg.as_str() {
                "--network" => network = Some(NetworkMode::from_flag(&value("--network")?)?),
                "--data-dir" => data_root = PathBuf::from(value("--data-dir")?),
                "--esplora" => {
                    let text = value("--esplora")?;
                    let before = esplora.len();
                    esplora.extend(text.split(',').map(str::trim).filter(|url| !url.is_empty()).map(String::from));
                    if esplora.len() == before {
                        return Err("--esplora needs a URL".to_string());
                    }
                }
                "--peer" => {
                    let text = value("--peer")?;
                    peers.extend(text.split(',').map(str::trim).filter(|peer| !peer.is_empty()).map(String::from));
                }
                "--public-fallback" => public_fallback = Some(true),
                "--no-public-fallback" => public_fallback = Some(false),
                "--confirmations" => {
                    let text = value("--confirmations")?;
                    let parsed: u32 = text
                        .parse()
                        .map_err(|_| "--confirmations must be a number".to_string())?;
                    if parsed == 0 {
                        return Err("--confirmations must be at least 1".to_string());
                    }
                    confirmations = Some(parsed);
                }
                "--poll-secs" => {
                    let text = value("--poll-secs")?;
                    poll_secs = text
                        .parse::<u64>()
                        .ok()
                        .filter(|secs| *secs > 0)
                        .ok_or_else(|| "--poll-secs must be a positive number".to_string())?;
                }
                "--escrow-vendor" => {
                    let text = value("--escrow-vendor")?;
                    let (seller, url) = text
                        .split_once('=')
                        .ok_or_else(|| "--escrow-vendor needs SELLER=URL".to_string())?;
                    let url = crate::peer_http::sale_base(url)?;
                    escrow_vendors.push((seller.trim().to_string(), url));
                }
                "--moderator" => {
                    let url = value("--moderator")?;
                    moderator_url = Some(crate::peer_http::sale_base(&url)?);
                }
                "--moderate" => moderate = true,
                "--vault" => {
                    vault = match value("--vault")?.as_str() {
                        "keystore" => VaultChoice::Keystore,
                        "passphrase" => VaultChoice::Passphrase,
                        "file" => VaultChoice::File,
                        other => return Err(format!("--vault must be keystore, passphrase, or file, not {other:?}")),
                    }
                }
                "--shop-origin" => shop_origin = crate::http_guard::parse_shop_origin(&value("--shop-origin")?)?,
                "--no-pair-codes" => pair_codes = false,
                "--proxy" => proxy = Some(crate::egress::Egress::parse_proxy(&value("--proxy")?)?),
                "--no-rates" => live_rates = false,
                "--xmr-daemon" => {
                    let text = value("--xmr-daemon")?;
                    xmr_daemons.extend(text.split(',').map(str::trim).filter(|url| !url.is_empty()).map(String::from));
                }
                "--follow" => follows.push(crate::replicas::normalize(&value("--follow")?)?),
                "--local-shop" => {
                    let path = PathBuf::from(value("--local-shop")?);
                    if !path.join("index.html").is_file() {
                        return Err("--local-shop needs a directory that contains index.html".to_string());
                    }
                    local_shop = Some(path);
                }
                other if !other.starts_with('-') => {
                    port = other
                        .parse()
                        .map_err(|_| "port must be a number".to_string())?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        let network = network.unwrap_or(NetworkMode::Testnet4);
        let peers = peers
            .iter()
            .map(|peer| crate::chain_backend::parse_peer(peer, network))
            .collect::<Result<Vec<_>, _>>()?;
        let public_fallback = match public_fallback {
            Some(choice) => choice,
            None => network == NetworkMode::Testnet4 && peers.is_empty(),
        };
        crate::chain_backend::ChainPlan::choose(network, &peers, &esplora, public_fallback)?;
        if !xmr_daemons.is_empty() {
            openbazaar_xmr::DaemonConfig::new(network.monero(), &xmr_daemons, proxy.clone())?;
        }
        lightning.plan(network, proxy.as_deref())?;
        Ok(Self {
            port,
            network,
            data_root,
            esplora_urls: esplora,
            peers,
            public_fallback,
            confirmations: confirmations.unwrap_or_else(|| network.default_confirmations()),
            poll_secs,
            escrow_vendors,
            moderator_url,
            moderate,
            vault,
            shop_origin,
            pair_codes,
            proxy,
            live_rates,
            xmr_daemons,
            lightning,
            follows,
            local_shop,
        })
    }

    /// The Monero daemons these flags select, or `None` when Monero is off.
    pub fn xmr_config(&self) -> Option<openbazaar_xmr::DaemonConfig> {
        (!self.xmr_daemons.is_empty()).then(|| {
            openbazaar_xmr::DaemonConfig::new(self.network.monero(), &self.xmr_daemons, self.proxy.clone())
                .expect("parse already checked --xmr-daemon")
        })
    }

    pub fn data_dir(&self) -> PathBuf {
        self.network.data_dir(&self.data_root)
    }

    /// Escrow peers for this node. It advertises its own loopback port.
    pub fn escrow_config(&self) -> crate::EscrowConfig {
        crate::EscrowConfig {
            mailbox_url: Some(format!("http://127.0.0.1:{}", self.port)),
            vendors: self.escrow_vendors.iter().cloned().collect(),
            moderator_url: self.moderator_url.clone(),
            moderates: self.moderate,
        }
    }

    pub fn egress(&self) -> crate::Egress {
        crate::Egress {
            proxy: self.proxy.clone(),
            live_rates: self.live_rates,
        }
    }

    /// The chain backend these flags select.
    pub fn chain_plan(&self) -> crate::ChainPlan {
        crate::ChainPlan::choose(self.network, &self.peers, &self.esplora_urls, self.public_fallback)
            .expect("parse already checked the chain flags")
    }

    /// True when chain queries may go to a server that is not on this machine or LAN.
    pub fn uses_public_server(&self) -> bool {
        self.chain_plan().esplora_servers().iter().any(|url| !is_private_url(url))
    }
}

pub(crate) fn is_private_url(url: &str) -> bool {
    // A backslash is not a path separator here. The URL parser accepts some of
    // them, and a split on "://" would call the wrong host private.
    if url.contains('\\') {
        return false;
    }
    let Ok(parsed) = url::Url::parse(url.trim()) else { return false };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else { return false };
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".onion") {
        return true;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        _ => false,
    }
}

/// Parse an address and require that it belongs to `network`.
pub(crate) fn address_for(text: &str, network: NetworkMode) -> Result<Address, String> {
    let unchecked = Address::from_str(text.trim()).map_err(|_| "That is not a bitcoin address.".to_string())?;
    unchecked
        .require_network(network.bitcoin())
        .map_err(|_| format!("That address is not a {network} address."))
}

pub(crate) fn offer_for(text: &str, network: NetworkMode) -> Result<Bolt12Offer, String> {
    let offer = Bolt12Offer::parse(text).map_err(|err| err.to_string())?;
    if !offer.is_for(network.bitcoin()) {
        return Err(format!("That offer is not for {network}."));
    }
    Ok(offer)
}

pub(crate) fn code_for(text: &str, network: NetworkMode) -> Result<SilentPaymentCode, String> {
    let code = SilentPaymentCode::parse(text).map_err(|err| err.to_string())?;
    if !code.is_for(network.bitcoin()) {
        return Err(format!("That silent-payment code is not for {network}."));
    }
    Ok(code)
}

/// BIP 21 URI for one address and amount.
pub(crate) fn bip21(address: &Address, amount: Amount) -> String {
    let sats = amount.to_sat();
    let whole = sats / 100_000_000;
    let fraction = sats % 100_000_000;
    let btc = if fraction == 0 {
        whole.to_string()
    } else {
        let digits = format!("{fraction:08}");
        format!("{whole}.{}", digits.trim_end_matches('0'))
    };
    format!("bitcoin:{address}?amount={btc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|text| text.to_string()).collect()
    }

    #[test]
    fn testnet4_is_the_default_and_mainnet_needs_the_exact_flag() {
        let options = ServeOptions::parse(&[]).unwrap();
        assert_eq!(options.network, NetworkMode::Testnet4);
        assert_eq!(options.port, 8787);
        assert_eq!(options.confirmations, 1);
        assert_eq!(options.data_dir(), PathBuf::from("data/testnet4"));
        assert!(options.esplora_urls.is_empty());
        assert_eq!(
            options.chain_plan().esplora_servers(),
            ["https://mempool.space/testnet4/api", "https://blockstream.info/testnet4/api"],
            "testnet4 falls back to public Esplora only while no filter peer answers"
        );

        let main = ServeOptions::parse(&args(&["9100", "--network", "mainnet"])).unwrap();
        assert_eq!(main.network, NetworkMode::Mainnet);
        assert_eq!(main.port, 9100);
        assert_eq!(main.data_dir(), PathBuf::from("data/mainnet"));
        assert!(!main.uses_public_server(), "mainnet uses compact filters and no public server by default");

        for bad in [
            &["--network"][..],
            &["--network", "bitcoin"][..],
            &["--network", "Mainnet"][..],
            &["--network", "main"][..],
            &["--network", "testnet"][..],
            &["--mainnet"][..],
        ] {
            assert!(ServeOptions::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn local_surface_flags_parse() {
        let options = ServeOptions::parse(&[]).unwrap();
        assert_eq!(options.vault, VaultChoice::Keystore, "the key goes to the OS keystore unless told otherwise");
        assert_eq!(options.shop_origin, "http://127.0.0.1:4173");
        assert!(options.pair_codes && options.live_rates && options.proxy.is_none());
        let options = ServeOptions::parse(&args(&[
            "--vault",
            "passphrase",
            "--shop-origin",
            "http://localhost:19006",
            "--no-pair-codes",
            "--proxy",
            "socks5://127.0.0.1:9050",
            "--no-rates",
        ]))
        .unwrap();
        assert_eq!(options.vault, VaultChoice::Passphrase);
        assert_eq!(options.shop_origin, "http://localhost:19006");
        assert!(!options.pair_codes && !options.live_rates);
        assert_eq!(options.proxy.as_deref(), Some("socks5h://127.0.0.1:9050"));
        for bad in [&["--vault", "plain"][..], &["--shop-origin", "https://evil.example"], &["--proxy", "http://127.0.0.1:8080"]] {
            assert!(ServeOptions::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn private_servers_are_recognized() {
        for url in [
            "http://127.0.0.1:3002",
            "http://localhost:3000/api",
            "http://192.168.1.20:3002",
            "http://[::1]:3002",
            "http://node.local/api",
        ] {
            let options = ServeOptions::parse(&args(&["--esplora", url])).unwrap();
            assert!(!options.uses_public_server(), "{url}");
        }
        let public = ServeOptions::parse(&args(&["--esplora", "https://blockstream.info/api"])).unwrap();
        assert!(public.uses_public_server());
        assert!(is_private_url("http://mempool.space@127.0.0.1/api"), "the host is the loopback address");
        assert!(!is_private_url("http://127.0.0.1@mempool.space/api"), "the host is mempool.space");
        assert!(!is_private_url("http://127.0.0.1\\@127.0.0.1/"));
        assert!(!is_private_url("not a url"));
        assert!(is_private_url("http://10.1.2.3:3002"));
        assert!(is_private_url("http://[fd00::1]/api"));
        let mixed = ServeOptions::parse(&args(&["--esplora", "http://127.0.0.1:3002, https://mempool.space/api"])).unwrap();
        assert!(mixed.uses_public_server());
    }

    #[test]
    fn esplora_servers_are_an_ordered_list() {
        let options = ServeOptions::parse(&args(&[
            "--esplora",
            "http://127.0.0.1:3002,http://node.local/api",
            "--esplora",
            "http://192.168.1.20:3002",
        ]))
        .unwrap();
        assert_eq!(
            options.esplora_urls,
            ["http://127.0.0.1:3002", "http://node.local/api", "http://192.168.1.20:3002"]
        );
        assert!(!options.uses_public_server());
        assert!(ServeOptions::parse(&args(&["--esplora", " , "])).is_err());
        let main = ServeOptions::parse(&args(&["--network", "mainnet"])).unwrap();
        assert!(main.esplora_urls.iter().all(|url| !url.contains("testnet")));
    }

    #[test]
    fn chain_backend_flags() {
        use crate::ChainPlan;
        let main = ServeOptions::parse(&args(&["--network", "mainnet"])).unwrap();
        let ChainPlan::Filters { config, fallback } = main.chain_plan() else { panic!("filters are the default") };
        assert!(!config.only_these_peers && fallback.is_empty());

        let opted = ServeOptions::parse(&args(&["--network", "mainnet", "--public-fallback"])).unwrap();
        assert_eq!(opted.chain_plan().esplora_servers(), ["https://mempool.space/api"]);
        assert!(opted.uses_public_server());
        let quiet = ServeOptions::parse(&args(&["--no-public-fallback"])).unwrap();
        assert!(quiet.chain_plan().esplora_servers().is_empty() && !quiet.uses_public_server());

        let own = ServeOptions::parse(&args(&["--peer", "192.168.1.5:48333,node.local", "--peer", "[::1]:18444"])).unwrap();
        assert_eq!(
            own.peers,
            [("192.168.1.5".to_string(), 48333), ("node.local".to_string(), 48333), ("::1".to_string(), 18444)]
        );
        let ChainPlan::Filters { config, fallback } = own.chain_plan() else { panic!("--peer uses filters") };
        assert!(config.only_these_peers && fallback.is_empty(), "own node: no seeds, no fallback");
        assert!(!own.uses_public_server());
        let main_peer = ServeOptions::parse(&args(&["--network", "mainnet", "--peer", "10.0.0.2"])).unwrap();
        assert_eq!(main_peer.peers, [("10.0.0.2".to_string(), 8333)]);

        for bad in [
            &["--peer", "10.0.0.2", "--esplora", "http://127.0.0.1:3002"][..],
            &["--peer", "10.0.0.2", "--public-fallback"],
            &["--peer", "10.0.0.2:0"],
            &["--peer", "a:b:c"],
            &["--peer", "user@host:1"],
        ] {
            assert!(ServeOptions::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn monero_is_off_by_default_and_never_public_without_tor_on_mainnet() {
        use openbazaar_xmr::XmrNetwork;
        assert_eq!(NetworkMode::Testnet4.monero(), XmrNetwork::Stagenet);
        assert_eq!(NetworkMode::Mainnet.monero(), XmrNetwork::Mainnet);
        assert!(ServeOptions::parse(&[]).unwrap().xmr_config().is_none(), "no default Monero node");
        let own = ServeOptions::parse(&args(&["--xmr-daemon", "http://127.0.0.1"])).unwrap();
        let config = own.xmr_config().unwrap();
        assert_eq!((config.network, config.nodes[0].url.as_str()), (XmrNetwork::Stagenet, "http://127.0.0.1:38081"));
        let main = ["--network", "mainnet", "--xmr-daemon", "http://node.example.org:18089"];
        assert!(ServeOptions::parse(&args(&main)).unwrap_err().contains("Tor"));
        let stagenet = ["--xmr-daemon", "http://node.example.org:38081"];
        assert!(ServeOptions::parse(&args(&stagenet)).unwrap_err().contains("Tor"));
        let tor = ServeOptions::parse(&args(&[&main[..], &["--proxy", "socks5h://127.0.0.1:9050"]].concat())).unwrap();
        assert_eq!(tor.xmr_config().unwrap().privacy(), "remote-tor");
    }

    #[test]
    fn addresses_from_the_wrong_network_are_rejected() {
        let mainnet = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let testnet = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
        let regtest = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";
        assert!(address_for(mainnet, NetworkMode::Mainnet).is_ok());
        assert!(address_for(mainnet, NetworkMode::Testnet4).is_err());
        assert!(address_for(testnet, NetworkMode::Testnet4).is_ok());
        assert!(address_for(testnet, NetworkMode::Mainnet).is_err());
        assert!(address_for(regtest, NetworkMode::Testnet4).is_err());
        assert!(address_for("not an address", NetworkMode::Testnet4).is_err());
    }

    #[test]
    fn bip21_writes_the_amount_in_bitcoin() {
        let address = address_for("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", NetworkMode::Testnet4).unwrap();
        assert_eq!(
            bip21(&address, Amount::from_sat(36_000)),
            "bitcoin:tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx?amount=0.00036"
        );
        assert!(bip21(&address, Amount::from_sat(100_000_000)).ends_with("amount=1"));
        assert!(bip21(&address, Amount::from_sat(1)).ends_with("amount=0.00000001"));
    }
}
