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

    pub fn default_esplora(self) -> &'static str {
        match self {
            NetworkMode::Testnet4 => "https://mempool.space/testnet4/api",
            NetworkMode::Mainnet => "https://mempool.space/api",
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

/// Everything `serve` needs to know before it opens the data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    pub port: u16,
    pub network: NetworkMode,
    pub data_root: PathBuf,
    pub esplora_url: String,
    pub confirmations: u32,
    pub poll_secs: u64,
}

impl ServeOptions {
    /// Parse the arguments after `serve`.
    ///
    /// With no `--network`, the node runs on testnet4. Mainnet is never a fallback.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut port = 8787u16;
        let mut network = None;
        let mut data_root = PathBuf::from("data");
        let mut esplora = None;
        let mut confirmations = None;
        let mut poll_secs = 30u64;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| {
                iter.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--network" => network = Some(NetworkMode::from_flag(&value("--network")?)?),
                "--data-dir" => data_root = PathBuf::from(value("--data-dir")?),
                "--esplora" => esplora = Some(value("--esplora")?),
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
                other if !other.starts_with('-') => {
                    port = other
                        .parse()
                        .map_err(|_| "port must be a number".to_string())?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        let network = network.unwrap_or(NetworkMode::Testnet4);
        Ok(Self {
            port,
            network,
            data_root,
            esplora_url: esplora.unwrap_or_else(|| network.default_esplora().to_string()),
            confirmations: confirmations.unwrap_or_else(|| network.default_confirmations()),
            poll_secs,
        })
    }

    pub fn data_dir(&self) -> PathBuf {
        self.network.data_dir(&self.data_root)
    }

    /// True when chain queries go to a server that is not on this machine or LAN.
    pub fn uses_public_server(&self) -> bool {
        !is_private_url(&self.esplora_url)
    }
}

fn is_private_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    };
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".onion") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
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
        assert_eq!(options.esplora_url, "https://mempool.space/testnet4/api");

        let main = ServeOptions::parse(&args(&["9100", "--network", "mainnet"])).unwrap();
        assert_eq!(main.network, NetworkMode::Mainnet);
        assert_eq!(main.port, 9100);
        assert_eq!(main.data_dir(), PathBuf::from("data/mainnet"));
        assert!(main.uses_public_server());

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
