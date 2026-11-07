//! Lightning direct pay on this node, over [`openbazaar_lightning`].
//!
//! One LDK node per OpenBazaar node, for the node wallet (`wallets/node`).
//! It is the seller's node for the own store and for the demo stores this
//! node plays on testnet4, and the buyer's node when the user pays from it.
//!
//! | tier | file | sealed with | holds |
//! |---|---|---|---|
//! | receive | `wallets/node/ln-keys.obv` | vault key | the Lightning secret (`m/1984'/coin'/6'/0'`) |
//! | receive | `wallets/node/lightning/sealed/` | a key from that secret | LDK's channels, payments, wallet |
//! | — | `ln-settings.obv` | vault key | the LSP acknowledgement and demo stores' choice |
//!
//! A Lightning node must hold its keys to receive (it signs every channel
//! update), so the receive tier holds them: a locked node still issues
//! invoices, takes payments, and marks orders paid. Paying, opening and
//! closing channels, and sweeping LDK's on-chain funds are refused in this
//! node's API without the passphrase: a spend ticket for a payment, the
//! passphrase itself for channel work. That is a policy gate, not a key
//! gate; `docs/lightning.md` says so.
//!
//! Lightning is off until `ob-node serve --lightning`. Stores opt in one by
//! one (`PUT /ln/stores`), which signs the store record again with the
//! Lightning flag; the first opt-in with an LSP needs `acknowledgeLsp`.
//!
//! A mainnet node refuses Lightning ([`MAINNET_BLOCKED`]) unless `--proxy` is
//! set, so every public peer, Esplora, and gossip dial runs over Tor.

mod checkout;
mod http;
pub(crate) mod options;
mod pay;
#[cfg(test)]
mod regtest_tests;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bitcoin::secp256k1::SecretKey;
use openbazaar_lightning::{LdkRail, LightningRail, LnConfig};
use openbazaar_protocol::{Catalog, Store};
use serde_json::{json, Value};

use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::wallet_keys;
use crate::{Error, Node};

pub(crate) use checkout::{add_ln_flags, ln_rail_of};
pub(crate) use http::response;
pub(crate) use pay::LnTerms;
pub use options::LnFlags;

const KEYS_FILE: &str = "ln-keys.obv";
const SETTINGS_FILE: &str = "ln-settings.obv";
const STATE_DIR: &str = "lightning";
/// After a failed start, wait this long before the next try.
const RETRY: Duration = Duration::from_secs(60);

/// What the user is told once, before a store first takes Lightning through an LSP.
pub(crate) const LSP_NOTICE: &str = "Lightning payments to your store reach you through a Lightning Service Provider (LSP). \
The LSP opens a channel to your node when the first payment arrives and keeps a fee for it. \
It sees the amount and time of every payment to you and the channel's size, and it must be online for a new channel to open. \
It never sees what was bought, who bought it, or the shipping address. You can name another LSP or open channels yourself.";

/// Why a mainnet node refuses Lightning, in every place it is asked for.
pub(crate) const MAINNET_BLOCKED: &str =
    "Lightning on mainnet is disabled until its connections run over Tor; it would reveal this node's IP address.";

#[derive(Default)]
struct Settings {
    lsp_acknowledged: bool,
    stores: BTreeSet<String>,
}

impl Settings {
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::Detail(format!("{SETTINGS_FILE} could not be read")))?;
        Ok(Self {
            lsp_acknowledged: value["lspAcknowledged"].as_bool().unwrap_or(false),
            stores: value["stores"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect(),
        })
    }

    fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(&json!({ "lspAcknowledged": self.lsp_acknowledged, "stores": self.stores })).unwrap_or_default()
    }
}

pub(crate) struct LnNode {
    /// Unset: Lightning is off on this node.
    config: OnceLock<LnConfig>,
    vault: Vault,
    wallet_dir: PathBuf,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    rail: OnceLock<Arc<dyn LightningRail>>,
    error: Mutex<Option<String>>,
    next_try: Mutex<Option<Instant>>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn read_settings(path: &Path, vault: &Vault) -> Result<Settings, Error> {
    match vault.read_sealed(path)? {
        Some(bytes) => Settings::decode(&bytes),
        None => Ok(Settings::default()),
    }
}

impl LnNode {
    pub(crate) fn open(dir: &Path, vault: &Vault) -> Result<Self, Error> {
        let wallet_dir = dir.join("wallets").join("node");
        let settings_path = dir.join(SETTINGS_FILE);
        let settings = read_settings(&settings_path, vault)?;
        Ok(Self {
            config: OnceLock::new(),
            vault: vault.clone(),
            wallet_dir,
            settings_path,
            settings: Mutex::new(settings),
            rail: OnceLock::new(),
            error: Mutex::new(None),
            next_try: Mutex::new(None),
        })
    }

    pub(crate) fn config(&self) -> Option<&LnConfig> {
        self.config.get()
    }

    pub(crate) fn rail(&self) -> Option<&Arc<dyn LightningRail>> {
        self.rail.get()
    }

    /// The Lightning secret from the receive tier, if this node has written it.
    fn secret(&self) -> Result<Option<zeroize::Zeroizing<[u8; 32]>>, Error> {
        match self.vault.read_sealed(&self.wallet_dir.join(KEYS_FILE))? {
            Some(bytes) => {
                let bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::Detail(format!("{KEYS_FILE} could not be read")))?;
                Ok(Some(zeroize::Zeroizing::new(bytes)))
            }
            None => Ok(None),
        }
    }

    fn write_secret(&self, seed: &Seed) -> Result<(), Error> {
        self.vault.write_sealed(&self.wallet_dir.join(KEYS_FILE), &seed.lightning_secret()[..])
    }

    fn lsp_acknowledged(&self) -> bool {
        guard(&self.settings).lsp_acknowledged
    }

    fn save_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut settings = guard(&self.settings);
        change(&mut settings);
        self.vault.write_sealed(&self.settings_path, &settings.encode()).map_err(|err| err.to_string())
    }
}

/// Sign each demo store that opted in to Lightning again with the flag.
pub(crate) fn apply_signed_accepts(dir: &Path, vault: &Vault, catalog: &mut Catalog, network: NetworkMode) -> Result<(), Error> {
    if network == NetworkMode::Mainnet {
        return Ok(());
    }
    let settings = read_settings(&dir.join(SETTINGS_FILE), vault)?;
    for store in &mut catalog.stores {
        if !settings.stores.contains(&store.name) {
            continue;
        }
        let Some(secret) = crate::fixture_secret(store, network) else { continue };
        *store = with_lightning(store, true, &secret).map_err(Error::Protocol)?;
    }
    Ok(())
}

fn with_lightning(store: &Store, accept: bool, secret: &SecretKey) -> Result<Store, openbazaar_protocol::Error> {
    let mut next = store.clone();
    if let Some(details) = next.details.as_mut() {
        details.accepts_lightning = accept;
    }
    next.resign(secret)
}

/// Why a store's Lightning choice could not change.
pub(crate) enum AcceptError {
    Unknown(String),
    Off,
    /// The first opt-in with an LSP: the user has not seen [`LSP_NOTICE`].
    NeedsAcknowledgement,
    Locked,
    Failed(String),
    Mainnet,
}

impl Node {
    /// Turn Lightning on with `config` (from [`LnFlags::plan`]). LDK state
    /// goes under `wallets/node/lightning/`; the node starts it in `serve`
    /// once the Lightning secret is in the receive tier.
    pub fn enable_lightning(&self, mut config: LnConfig) -> Result<(), String> {
        if config.proxy.is_none()
            && (self.network == NetworkMode::Mainnet
                || config.network == openbazaar_lightning::ldk_node::bitcoin::Network::Bitcoin)
        {
            return Err(MAINNET_BLOCKED.into());
        }
        if let Some(peer) = config.dials_in_the_clear() {
            return Err(format!(
                "Lightning would connect to {peer} in the clear, and that host would see this machine's IP"
            ));
        }
        config.storage_dir = self.ln.wallet_dir.join(STATE_DIR);
        self.ln.config.set(config).map_err(|_| "Lightning is already configured".to_string())
    }

    /// Use `rail` for Lightning (tests; `serve` starts LDK itself).
    pub fn set_lightning(&self, rail: Arc<dyn LightningRail>) -> Result<(), String> {
        if self.ln_blocked() {
            return Err(MAINNET_BLOCKED.into());
        }
        self.ln.rail.set(rail).map_err(|_| "the Lightning node is already running".to_string())
    }

    /// `true` on mainnet until Lightning is configured to dial through `--proxy`.
    pub(crate) fn ln_blocked(&self) -> bool {
        self.network == NetworkMode::Mainnet && self.ln.config().is_none_or(|config| config.proxy.is_none())
    }

    /// What the store named `seller` signed about Lightning. Always `false`
    /// on mainnet, whatever a record says.
    pub(crate) fn accepts_lightning(&self, seller: &str) -> bool {
        !self.ln_blocked()
            && self
                .catalog()
                .stores
                .iter()
                .find(|store| store.name == seller)
                .and_then(|store| store.details.as_ref())
                .is_some_and(|details| details.accepts_lightning)
    }

    /// Stores whose invoices this node's Lightning node issues: the own
    /// store, and the demo stores it plays.
    pub(crate) fn ln_sellers(&self) -> Vec<String> {
        let catalog = self.catalog();
        let mut sellers: Vec<String> = catalog
            .stores
            .iter()
            .filter(|store| store.id == self.own_store || (self.store_wallets.contains_key(&store.name) && self.network == NetworkMode::Testnet4))
            .map(|store| store.name.clone())
            .collect();
        sellers.sort();
        sellers.dedup();
        sellers
    }

    pub(crate) fn set_accepts_lightning(&self, seller: &str, accept: bool, acknowledge: bool) -> Result<(), AcceptError> {
        if self.ln_blocked() {
            return Err(AcceptError::Mainnet);
        }
        let Some(config) = self.ln.config() else { return Err(AcceptError::Off) };
        if !self.ln_sellers().iter().any(|name| name == seller) {
            return Err(AcceptError::Unknown("This node does not issue invoices for that store.".into()));
        }
        if accept && config.lsp.is_some() && !self.ln.lsp_acknowledged() {
            if !acknowledge {
                return Err(AcceptError::NeedsAcknowledgement);
            }
            self.ln.save_settings(|settings| settings.lsp_acknowledged = true).map_err(AcceptError::Failed)?;
        }
        let store = self.seller_store(seller).map_err(AcceptError::Unknown)?;
        if store.details.is_none() {
            return Err(AcceptError::Unknown("That store's record cannot carry a Lightning choice.".into()));
        }
        let secret = match self.store_secret(&store.id) {
            Some(secret) => secret,
            None if store.id == self.own_store => return Err(AcceptError::Locked),
            None => return Err(AcceptError::Unknown("This node does not hold that store's key.".into())),
        };
        if store.id == self.own_store {
            let rating = store.rating.unwrap_or(openbazaar_protocol::ContentRating::General);
            let rails = crate::own_store::Rails { lightning: accept, ..crate::own_store::Rails::of(Some(&store)) };
            return self.publish_own(rating, rails, &secret).map_err(|err| AcceptError::Failed(err.to_string()));
        }
        let signed = with_lightning(&store, accept, &secret).map_err(|err| AcceptError::Failed(err.to_string()))?;
        self.ln
            .save_settings(|settings| {
                if accept {
                    settings.stores.insert(seller.to_string());
                } else {
                    settings.stores.remove(seller);
                }
            })
            .map_err(AcceptError::Failed)?;
        let mut catalog = self.catalog_mut();
        if let Some(old) = catalog.stores.iter_mut().find(|old| old.id == signed.id) {
            *old = signed;
        }
        Ok(())
    }

    pub(crate) fn ln_stores_value(&self) -> Value {
        if self.ln_blocked() {
            return blocked_value();
        }
        json!({
            "enabled": self.ln.config().is_some(),
            "lspAcknowledged": self.ln.lsp_acknowledged(),
            "lspNotice": self.ln.config().and_then(|config| config.lsp.as_ref()).map(|_| LSP_NOTICE),
            "stores": self.ln_sellers().iter().map(|seller| json!({
                "seller": seller,
                "own": self.catalog().stores.iter().any(|store| store.name == *seller && store.id == self.own_store),
                "acceptsLightning": self.accepts_lightning(seller),
            })).collect::<Vec<_>>(),
        })
    }

    /// `GET /ln/status`: the node, its LSP, balances, channels.
    pub(crate) fn ln_status_value(&self) -> Value {
        if self.ln_blocked() {
            return blocked_value();
        }
        let Some(config) = self.ln.config() else {
            return json!({ "enabled": false });
        };
        let mut value = json!({
            "enabled": true,
            "network": options::network_name(config.network),
            "chain": config.chain.describe(),
            "chainLocal": config.chain.is_local(),
            "gossip": match &config.gossip {
                openbazaar_lightning::GossipSource::P2p => "peers".to_string(),
                openbazaar_lightning::GossipSource::Rgs(url) => format!("rapid gossip sync {url}"),
            },
            "lsp": config.lsp.as_ref().map(|lsp| json!({
                "nodeId": lsp.peer.node_id.to_string(),
                "address": lsp.peer.address,
                "connected": false,
            })),
            "lspAcknowledged": self.ln.lsp_acknowledged(),
            "lspNotice": config.lsp.as_ref().map(|_| LSP_NOTICE),
            "maxLspFeeSatsFloor": config.min_lsp_fee_cap_sats,
            "running": false,
            "keys": if self.ln.secret().ok().flatten().is_some() { "ready" } else { "unlock-to-create" },
            "error": guard(&self.ln.error).clone(),
            "reveals": reveals(config, self.local.egress.proxy.is_some()),
        });
        if let Some(rail) = self.ln.rail() {
            let info = rail.info();
            let balances = rail.balances();
            value["running"] = json!(info.running);
            value["nodeId"] = json!(info.node_id);
            value["bestHeight"] = json!(info.best_height);
            value["lastSync"] = json!(info.last_wallet_sync.max(info.last_lightning_sync));
            if let (Some(lsp), Some((_, connected))) = (value["lsp"].as_object_mut(), info.lsp) {
                lsp.insert("connected".into(), json!(connected));
            }
            value["balances"] = json!({
                "lightningSats": balances.lightning_sats,
                "inboundSats": balances.inbound_sats,
                "onchainSats": balances.onchain_total_sats,
                "onchainSpendableSats": balances.onchain_spendable_sats,
            });
            value["channels"] = json!(rail.channels().iter().map(|channel| json!({
                "id": channel.id,
                "peer": channel.peer,
                "capacitySats": channel.capacity_sats,
                "outboundSats": channel.outbound_msat / 1_000,
                "inboundSats": channel.inbound_msat / 1_000,
                "ready": channel.ready,
                "usable": channel.usable,
                "fundingTxid": channel.funding_txid,
            })).collect::<Vec<_>>());
        }
        value
    }

    /// The short form `GET /wallet` carries.
    pub(crate) fn ln_wallet_value(&self) -> Value {
        if self.ln_blocked() {
            return blocked_value();
        }
        match (self.ln.config(), self.ln.rail()) {
            (None, _) => json!({ "enabled": false }),
            (Some(config), None) => json!({ "enabled": true, "running": false, "network": options::network_name(config.network) }),
            (Some(config), Some(rail)) => {
                let balances = rail.balances();
                json!({
                    "enabled": true,
                    "running": rail.info().running,
                    "network": options::network_name(config.network),
                    "lightningSats": balances.lightning_sats,
                    "inboundSats": balances.inbound_sats,
                    "onchainSats": balances.onchain_total_sats,
                })
            }
        }
    }

    /// `GET /network` `lightningBackend`.
    pub(crate) fn ln_backend_value(&self) -> Value {
        if self.ln_blocked() {
            return blocked_value();
        }
        match self.ln.config() {
            None => json!({ "enabled": false }),
            Some(config) => json!({
                "enabled": true,
                "network": options::network_name(config.network),
                "chain": config.chain.describe(),
                "privacy": if config.chain.is_local() { "own-node" } else { "public-server" },
                "lsp": config.lsp.as_ref().map(|lsp| lsp.peer.to_string()),
                "reveals": reveals(config, self.local.egress.proxy.is_some()),
            }),
        }
    }

    /// Write the Lightning secret to the receive tier if it is missing and a
    /// seed is at hand (before the first passphrase, or while unlocked).
    fn ln_fill_keys(&self) -> Result<bool, Error> {
        if self.ln.secret()?.is_some() {
            return Ok(true);
        }
        let dir = self.ln.wallet_dir.clone();
        let seed = match wallet_keys::vault_tier_seed(&dir, &self.ln.vault, self.network)? {
            Some(seed) => seed,
            None => match self.lock.spend_key() {
                Ok(key) => wallet_keys::spend_seed(&dir, &key, self.network)?,
                Err(_) => return Ok(false),
            },
        };
        self.ln.write_secret(&seed)?;
        Ok(true)
    }

    /// When `--remote` is on, publish an inbound onion and point LDK at it.
    ///
    /// The onion address exists only after the Tor client launches the
    /// service. LDK copies announcement addresses at start and cannot bind
    /// an onion, so this runs first: bind stays loopback or LAN, and the
    /// announcement is the onion. A failure here leaves LDK unstarted so a
    /// later retry can still set that announcement.
    fn ln_inbound(&self, config: &mut openbazaar_lightning::LnConfig) -> Result<(), String> {
        if !self.remote.onion_configured() || config.announce.is_some() {
            return Ok(());
        }
        let bind = config
            .listen
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", crate::remote::LIGHTNING_PORT));
        let local = inbound_target(&bind)?;
        let onion = self.remote.open_ln_onion(&self.data_dir, local)?;
        // Bind the address the onion forwards to, so LDK does not resolve a name.
        config.listen = Some(local.to_string());
        config.announce = Some(onion);
        Ok(())
    }

    /// Start the LDK node once its keys exist. Called from the Lightning thread.
    fn ln_start(&self) {
        let Some(mut config) = self.ln.config().cloned() else { return };
        if self.ln.rail().is_some() {
            return;
        }
        if guard(&self.ln.next_try).is_some_and(|at| Instant::now() < at) {
            return;
        }
        if let Err(err) = self.ln_inbound(&mut config) {
            eprintln!("ob-node: Lightning did not start: {err}");
            *guard(&self.ln.error) = Some(err);
            *guard(&self.ln.next_try) = Some(Instant::now() + RETRY);
            return;
        }
        let secret = match self.ln_fill_keys().and_then(|_| self.ln.secret()) {
            Ok(Some(secret)) => secret,
            Ok(None) => return,
            Err(err) => {
                *guard(&self.ln.error) = Some(err.to_string());
                return;
            }
        };
        let announced = config.announce.clone();
        match LdkRail::start(config, &secret) {
            Ok(rail) => {
                match &announced {
                    Some(onion) => eprintln!("ob-node: Lightning node {} accepts peers at {onion}", rail.info().node_id),
                    None => eprintln!("ob-node: Lightning node {} is running", rail.info().node_id),
                }
                *guard(&self.ln.error) = None;
                let _ = self.ln.rail.set(Arc::new(rail));
            }
            Err(err) => {
                eprintln!("ob-node: Lightning did not start: {err}");
                *guard(&self.ln.error) = Some(err.to_string());
                *guard(&self.ln.next_try) = Some(Instant::now() + RETRY);
            }
        }
    }

    /// The Lightning thread: start LDK when it can, then keep orders current.
    pub(crate) fn run_lightning(&self, stop: &AtomicBool) {
        if self.ln.config().is_none() && self.ln.rail().is_none() {
            return;
        }
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            self.ln_start();
            if self.ln.rail().is_some() {
                self.ln_update_orders();
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        if let Some(rail) = self.ln.rail() {
            rail.stop();
        }
    }
}

/// The loopback or LAN socket an onion may forward to. A name that is only
/// local (`localhost`, `*.local`) becomes `127.0.0.1` on that port.
fn inbound_target(listen: &str) -> Result<std::net::SocketAddr, String> {
    if !openbazaar_lightning::config::host_is_local(listen) {
        return Err(format!("Lightning will not bind {listen}: that is a public clearnet address"));
    }
    if let Ok(addr) = listen.parse::<std::net::SocketAddr>() {
        if addr.port() == 0 || !openbazaar_lightning::config::host_is_local(&addr.ip().to_string()) {
            return Err(format!("Lightning will not bind {listen}: that is a public clearnet address"));
        }
        return Ok(addr);
    }
    let port = listen
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("Lightning listen {listen} is not HOST:PORT"))?;
    Ok(std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)))
}

#[cfg(test)]
mod inbound_target_tests {
    use super::inbound_target;

    #[test]
    fn a_public_listen_is_not_the_onion_target() {
        for listen in ["1.2.3.4:9735", "0.0.0.0:9735", "8.8.8.8:9735", "203.0.113.9:9735", "[::]:9735"] {
            let err = inbound_target(listen).expect_err(listen);
            assert!(err.contains("public clearnet"), "{listen}: {err}");
        }
        assert_eq!(inbound_target("localhost:9735").unwrap().to_string(), "127.0.0.1:9735");
        assert_eq!(inbound_target("127.0.0.1:9736").unwrap().to_string(), "127.0.0.1:9736");
        let lan = inbound_target("192.168.1.9:9735").unwrap();
        assert!(matches!(lan.ip(), std::net::IpAddr::V4(ip) if ip.is_private()));
    }
}

/// What every Lightning status answer is on mainnet.
fn blocked_value() -> Value {
    json!({ "enabled": false, "blocked": MAINNET_BLOCKED })
}

/// Who learns what when this node uses Lightning.
fn reveals(config: &LnConfig, proxy: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(lsp) = &config.lsp {
        out.push(format!(
            "The LSP ({}) sees the amount and time of each payment to this node and the channels it opens.",
            lsp.peer.address
        ));
    }
    if proxy {
        out.push(
            "A public Lightning peer is reached through --proxy, so it does not see this machine's IP. A peer on this machine or the LAN is dialed directly.".to_string(),
        );
    } else {
        out.push(
            "Lightning peers on this machine or the LAN see the connection. A public peer is not contacted.".to_string(),
        );
    }
    if !config.chain.is_local() {
        let reach = if proxy {
            "It is reached through --proxy, so it does not see this machine's IP."
        } else {
            "It would see this machine's IP, so it is not contacted without --proxy."
        };
        out.push(format!(
            "The chain server ({}) sees the Lightning wallet's addresses and channel outputs. {reach}",
            config.chain.describe()
        ));
    }
    if let openbazaar_lightning::GossipSource::Rgs(url) = &config.gossip {
        if !openbazaar_lightning::config::host_is_local(url) {
            if proxy {
                out.push(format!("The gossip server ({url}) is reached through --proxy, so it does not see this machine's IP."));
            } else {
                out.push(format!("The gossip server ({url}) would see this machine's IP, so it is not contacted without --proxy."));
            }
        }
    }
    out
}
