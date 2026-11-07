//! Local shop node: signed fixture catalog, sealed orders, checkout rails,
//! a node wallet, and a chain watcher.
//!
//! The HTTP server is loopback-only. It does not dial Tor. It broadcasts a
//! transaction only when `POST /orders/{id}/pay` asks it to, with a spend
//! ticket the user confirmed, while the wallet is unlocked
//! ([`wallet_lock`], [`spend_tickets`]).

#![forbid(unsafe_code)]

mod addresses;
mod build_info;
mod buyer_orders;
mod chain;
mod chain_backend;
mod chain_filters;
mod coin_locks;
mod egress;
mod escrow_bump;
mod escrow_buyer;
mod escrow_fee;
mod escrow_fixture;
mod escrow_http;
mod escrow_limits;
mod escrow_mail;
mod escrow_moderator;
mod escrow_outbox;
mod escrow_rulings;
mod escrow_store;
mod escrow_vendor;
mod fiat;
mod http;
mod http_guard;
mod http_parse;
mod instructions;
mod labelers;
mod letter_auth;
mod mail_drop;
mod media;
mod network;
mod order_mail;
mod orders;
mod own_listings;
mod own_store;
mod pairing;
mod pay_bump;
mod pay_order;
mod public_http;
mod rates;
mod remote;
mod remote_orders;
mod replicas;
mod lock_http;
mod sales;
mod sales_http;
mod seed;
mod spend_tickets;
mod store_http;
mod store_profile;
mod stores;
mod vault;
mod vault_key;
mod vendor_labels;
mod wallet;
mod wallet_http;
mod wallet_keys;
mod wallet_lock;
mod wallet_session;
mod watch;
mod xmr;
mod lightning;

#[cfg(test)]
mod checkout_tests;
#[cfg(test)]
mod harness;
#[cfg(test)]
mod lock_tests;
#[cfg(test)]
mod lock_receive_tests;
#[cfg(test)]
mod multinode_tests;
#[cfg(test)]
mod multinode_durable_tests;
#[cfg(test)]
mod multinode_seller_tests;
#[cfg(test)]
mod own_store_tests;
#[cfg(test)]
mod pay_bump_tests;
#[cfg(test)]
mod payment_tests;
#[cfg(test)]
mod seller_store_tests;
#[cfg(test)]
mod stores_tests;
#[cfg(test)]
mod surface_tests;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use openbazaar_protocol::{
    fixture_catalog, fixture_keys, fixture_store_keys, Catalog, ContentRating, Error as ProtocolError, Listing, Moderator, StoreId,
};
use serde_json::{json, Value};
use tormail::Mailbox;

use addresses::AddressBook;
use buyer_orders::BuyerOrders;
use labelers::Labelers;
use rates::Rates;
use vault::Vault;
use vendor_labels::VendorLabels;
use wallet::NodeWallet;

pub use build_info::{BuildInfo, BUILD};
pub use chain::{AddressTx, ChainSource, ChainStatus, EsploraChain, OfflineChain, WalletRequest};
pub use chain_backend::ChainPlan;
pub use chain_filters::{ChainStore, FilterChain, FilterConfig};
pub use egress::Egress;
pub use escrow_fixture::MODERATOR_FEE_SATS;
pub use media::MAX_MEDIA;
pub use http::{http_response, serve_on};
pub use lightning::LnFlags;
pub use network::{NetworkMode, ServeOptions, VaultChoice};
pub use orders::OrderError;
pub use pay_order::PayError;
pub use remote::{devices_command, pair_command, RemoteConfig};
pub use vault_key::VaultMode;
pub use watch::{
    AWAITING_FUNDING, AWAITING_PAYMENT, AWAITING_TERMS, DISPUTED, FUNDED, FUNDED_WITHOUT_REFUND, FUNDING_SEEN, PAID,
    PAYMENT_SEEN, RELEASED, SETTLED,
};

/// How the node reaches the chain and when it calls a payment settled.
pub struct NodeConfig {
    pub network: NetworkMode,
    pub confirmations: u32,
    /// True when `chain` may ask a server outside this machine or LAN.
    /// `GET /network` reports what `chain.status()` says right now instead.
    pub public_server: bool,
    pub chain: Box<dyn ChainSource>,
    pub escrow: EscrowConfig,
}

/// Where this node's escrow peers are (testnet4 only). Loopback HTTP for now.
///
/// Without a `mailbox_url` and a vendor for the seller, escrow checkout is
/// refused: this node never plays both parties of one escrow.
#[derive(Clone, Debug, Default)]
pub struct EscrowConfig {
    /// Base URL of this node's own HTTP server (`http://127.0.0.1:PORT`). Peers reply here.
    pub mailbox_url: Option<String>,
    /// Seller name to the base URL of the node that holds that seller's escrow keys.
    pub vendors: BTreeMap<String, String>,
    /// Base URL of the node that runs the testnet4 fixture moderator.
    pub moderator_url: Option<String>,
    /// This node answers dispute letters as the testnet4 fixture moderator.
    pub moderates: bool,
}

/// The node's local attack surface and at-rest secrets.
#[derive(Clone, Debug)]
pub struct LocalConfig {
    pub vault: VaultMode,
    /// The one browser origin besides the desktop webview that may call the node.
    pub shop_origin: String,
    /// `POST /pair` prints one-time codes. The desktop app turns this off.
    pub pair_codes: bool,
    pub egress: Egress,
}

impl LocalConfig {
    /// `vault.key` in the data dir, the default shop origin, no proxy.
    /// Tests and headless tools; `ob-node serve` picks each field from flags.
    pub fn headless() -> Self {
        Self {
            vault: VaultMode::File,
            shop_origin: http_guard::DEFAULT_SHOP_ORIGIN.to_string(),
            pair_codes: true,
            egress: Egress::default(),
        }
    }
}

impl NodeConfig {
    /// No chain source. Status never moves and the pay endpoint reports the chain as unreachable.
    pub fn offline(network: NetworkMode) -> Self {
        Self {
            network,
            confirmations: network.default_confirmations(),
            public_server: false,
            chain: Box::new(OfflineChain),
            escrow: EscrowConfig::default(),
        }
    }
}

pub(crate) struct ChainSettings {
    pub confirmations: u32,
}

#[derive(Debug)]
pub enum Error {
    Protocol(ProtocolError),
    Detail(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Protocol(err) => write!(f, "{err}"),
            Error::Detail(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub struct Node {
    /// Written only by `POST /content-labels`.
    catalog: RwLock<Catalog>,
    /// This node's own store, signed with the store key from its seed.
    pub(crate) own_store: StoreId,
    /// The own store's served record is behind its inputs until the next unlock.
    pub(crate) own_pending: AtomicBool,
    /// The listings the own store signed, current and superseded.
    pub(crate) own_listings: Mutex<own_listings::OwnListings>,
    /// Stores followed on other nodes, as signed copies.
    pub(crate) replicas: Mutex<replicas::Replicas>,
    /// Sales to buyers on other nodes.
    pub(crate) sales: Mutex<sales::Sales>,
    /// Order letters and replies on their way to other nodes.
    pub(crate) order_outbox: escrow_outbox::Outbox,
    pub(crate) data_dir: PathBuf,
    pub(crate) media: media::Media,
    pub(crate) letter_guard: letter_auth::LetterGuard,
    pub(crate) vendor_labels: Mutex<VendorLabels>,
    pub(crate) mailbox: Mailbox,
    pub(crate) escrow: escrow_mail::Escrow,
    pairing_token: String,
    /// True only in the process that created `pairing.token`.
    pairing_fresh: bool,
    pub(crate) pairing_codes: pairing::PairingCodes,
    pub(crate) mail_drop: mail_drop::MailDrop,
    pub(crate) local: LocalConfig,
    rates: Rates,
    pub(crate) addresses: Mutex<AddressBook>,
    pub(crate) buyer_orders: Mutex<BuyerOrders>,
    pub(crate) labelers: Mutex<Labelers>,
    pub(crate) network: NetworkMode,
    pub(crate) settings: ChainSettings,
    pub(crate) chain: Box<dyn ChainSource>,
    /// The buyer's wallet: what `GET /wallet` shows and `POST /orders/{id}/pay` spends.
    buyer_wallet: Mutex<NodeWallet>,
    /// One wallet per fixture store, keyed by seller name. The node plays these vendors.
    pub(crate) store_wallets: BTreeMap<String, Mutex<NodeWallet>>,
    pub(crate) pay_jobs: pay_order::PayJobs,
    /// The spend tier: closed until `POST /unlock`, and again on lock.
    pub(crate) lock: wallet_lock::WalletLock,
    /// Monero direct pay, off until [`Node::set_xmr_chain`].
    pub(crate) xmr: xmr::XmrNode,
    /// Paired devices and the home node this node is a client of.
    pub(crate) remote: remote::Remote,
    pub(crate) ln: lightning::LnNode,
}

impl Node {
    /// Open `dir` on testnet4 with no chain source. Tests and offline tools use this.
    pub fn open(dir: impl Into<PathBuf>, catalog: Catalog) -> Result<Self, Error> {
        Self::open_with(dir, catalog, NodeConfig::offline(NetworkMode::Testnet4))
    }

    /// Open one network's data directory (`data/testnet4`, `data/mainnet`).
    ///
    /// The catalog must carry payment codes for `config.network`. The
    /// mailbox lives in `mailbox/`, wallets in `wallets/`, and the vault key,
    /// pairing token, address book, and orders at the top of `dir`.
    pub fn open_with(dir: impl Into<PathBuf>, catalog: Catalog, config: NodeConfig) -> Result<Self, Error> {
        Self::open_local(dir, catalog, config, LocalConfig::headless())
    }

    /// [`Node::open_with`] with the vault mode, allowed shop origin, pairing
    /// codes, and proxy chosen by the caller. Refuses a data dir whose secret
    /// files other users can read.
    pub fn open_local(
        dir: impl Into<PathBuf>,
        mut catalog: Catalog,
        config: NodeConfig,
        local: LocalConfig,
    ) -> Result<Self, Error> {
        let dir = dir.into();
        let network = config.network;
        check_catalog_network(&catalog, network)?;
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        vault_key::check_private(&dir)?;
        let vault = Vault::open(&dir, &local.vault)?;
        let wallets = dir.join("wallets");
        let buyer_wallet = NodeWallet::open(&wallets, "node", &vault, network)?;
        let moderators: Vec<[u8; 32]> =
            (!catalog.moderator.name.is_empty()).then_some(catalog.moderator.id).into_iter().collect();
        xmr::apply_signed_accepts(&dir, &vault, &mut catalog, network)?;
        lightning::apply_signed_accepts(&dir, &vault, &mut catalog, network)?;
        let own = own_store::open(&dir, &vault, &buyer_wallet, network, &moderators)?;
        let own_store = own.id;
        catalog.stores.retain(|store| store.id != own_store);
        catalog.stores.extend(own.record);
        let own_secret = wallet_keys::vault_tier_seed(&buyer_wallet.dir, &vault, network)?.map(|seed| seed.store_signing_key());
        let secrets: Vec<_> = catalog
            .stores
            .iter()
            .filter_map(|store| {
                if store.id == own_store {
                    own_secret.map(|secret| (store.id, secret))
                } else {
                    fixture_secret(store, network).map(|secret| (store.id, secret))
                }
            })
            .collect();
        let vendor_labels = VendorLabels::open(&dir, &mut catalog, &secrets)?;
        if let Some(record) = catalog.store(&own_store) {
            own_store::write_cached(&dir, &vault, record)?;
        }
        let mailbox = Mailbox::open(dir.join("mailbox")).map_err(|err| Error::Detail(err.to_string()))?;
        let escrow_inbox = Mailbox::open(dir.join("escrow-mail")).map_err(|err| Error::Detail(err.to_string()))?;
        let (pairing_token, pairing_fresh) = load_pairing(&dir)?;
        let addresses = AddressBook::open(&dir, vault.clone())?;
        let buyer_orders = BuyerOrders::open(&dir, vault.clone(), network.as_str())?;
        let labelers = Labelers::open(&dir, vault.clone())?;
        let mut store_wallets = BTreeMap::new();
        for listing in &catalog.listings {
            if store_wallets.contains_key(&listing.seller) {
                continue;
            }
            let name = format!("store-{}", slug(&listing.seller));
            let wallet = NodeWallet::open(&wallets, &name, &vault, network)?;
            store_wallets.insert(listing.seller.clone(), Mutex::new(wallet));
        }
        // After the store wallets, which are only for the demo sellers, and
        // after the vendor labels, which index the demo listings.
        let own_listings = own_listings::OwnListings::open(&dir, vault.clone(), &own_store)?;
        own_listings::serve_at_boot(&mut catalog, &own_listings);
        let replicas = replicas::Replicas::open(&dir, vault.clone())?;
        let demo_sellers: Vec<String> = store_wallets.keys().cloned().collect();
        Self::serve_replicas_at_boot(&replicas, &mut catalog, &own_store, &demo_sellers);
        let lock = wallet_lock::WalletLock::open(&dir, vault.clone(), &local.vault)?;
        let xmr_stores: Vec<(String, String)> =
            store_wallets.keys().map(|seller| (seller.clone(), format!("store-{}", slug(seller)))).collect();
        let xmr = xmr::XmrNode::open(&dir, &vault, network, &xmr_stores, local.egress.clone())?;
        let wallet_tip = buyer_wallet.wallet.latest_checkpoint().block_id();
        config
            .chain
            .open(ChainStore { dir: dir.join("filters"), vault: vault.clone(), wallet_tip: Some(wallet_tip) })
            .map_err(Error::Detail)?;
        let node = Self {
            catalog: RwLock::new(catalog),
            own_store,
            own_pending: AtomicBool::new(own.pending),
            own_listings: Mutex::new(own_listings),
            replicas: Mutex::new(replicas),
            sales: Mutex::new(sales::Sales::open(&dir, vault.clone())?),
            order_outbox: order_mail::open_outbox(&dir, vault.clone())?,
            data_dir: dir.clone(),
            media: media::Media::open(dir.join("media"), network)?,
            letter_guard: letter_auth::LetterGuard::open(&dir)?,
            vendor_labels: Mutex::new(vendor_labels),
            mailbox,
            escrow: escrow_mail::Escrow::open(config.escrow, escrow_inbox, &dir, &vault)?,
            pairing_token,
            pairing_fresh,
            pairing_codes: pairing::PairingCodes::new(local.pair_codes),
            mail_drop: mail_drop::MailDrop::new(),
            rates: Rates::new(local.egress.clone()),
            local,
            addresses: Mutex::new(addresses),
            buyer_orders: Mutex::new(buyer_orders),
            labelers: Mutex::new(labelers),
            network,
            settings: ChainSettings {
                confirmations: config.confirmations,
            },
            chain: config.chain,
            buyer_wallet: Mutex::new(buyer_wallet),
            store_wallets,
            pay_jobs: pay_order::PayJobs::new(),
            lock,
            xmr,
            remote: remote::Remote::open(&dir, vault.clone())?,
            ln: lightning::LnNode::open(&dir, &vault)?,
        };
        node.drop_uncommitted_spend()?;
        Ok(node)
    }

    pub fn network(&self) -> NetworkMode {
        self.network
    }

    pub(crate) fn catalog(&self) -> RwLockReadGuard<'_, Catalog> {
        self.catalog.read().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn catalog_mut(&self) -> RwLockWriteGuard<'_, Catalog> {
        self.catalog.write().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn buyer_wallet(&self) -> MutexGuard<'_, NodeWallet> {
        self.buyer_wallet.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn store_wallet(&self, seller: &str) -> Result<MutexGuard<'_, NodeWallet>, OrderError> {
        self.store_wallets
            .get(seller)
            .map(|wallet| wallet.lock().unwrap_or_else(|err| err.into_inner()))
            .ok_or(OrderError::Invalid("That seller has no wallet on this node."))
    }

    pub(crate) fn orders(&self) -> MutexGuard<'_, BuyerOrders> {
        self.buyer_orders.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn rates_value(&self) -> Value {
        let mut value = self.rates.value();
        value["xmr"] = self.xmr.rate.value(&value);
        value
    }

    pub fn mailbox_path(&self) -> &Path {
        self.mailbox.path()
    }

    pub(crate) fn pairing_token(&self) -> &str {
        &self.pairing_token
    }

    /// The token, only when this process just wrote it.
    ///
    /// `main` prints that value once. A later boot loads the same file and
    /// returns `None`, so the secret is not written to the log again.
    pub fn fresh_pairing_token(&self) -> Option<&str> {
        self.pairing_fresh.then_some(self.pairing_token.as_str())
    }

    /// Serve HTTP on this thread. The chain watcher (every `poll`), the pay
    /// worker, the escrow mail loop, and the auto-lock each run on their own
    /// thread, so chain and peer calls never block HTTP.
    pub fn serve(&self, port: u16, poll: Duration) -> std::io::Result<()> {
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| self.run_watcher(poll, &stop));
            scope.spawn(|| self.run_pay_worker(&stop));
            scope.spawn(|| self.run_escrow_mail(&stop));
            scope.spawn(|| self.run_auto_lock(&stop));
            scope.spawn(|| self.run_xmr(poll, &stop));
            scope.spawn(|| remote::run(self, port, &stop));
            scope.spawn(|| self.run_lightning(&stop));
            scope.spawn(|| self.run_order_mail(&stop));
            scope.spawn(|| self.run_replicas(poll.min(Duration::from_secs(30)), &stop));
            let result = http::serve(self, port);
            stop.store(true, Ordering::Relaxed);
            result
        })
    }
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
        .collect()
}

/// The fixture catalog with store payment codes for `network`.
/// Exit the process once stdin reaches end of file.
///
/// The desktop app holds the write end of this pipe. The OS closes it when the
/// app quits or crashes, so the node does not outlive the app.
pub fn exit_when_stdin_closes() {
    std::thread::spawn(|| {
        let mut sink = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(std::io::Read::read(&mut stdin, &mut sink), Ok(n) if n > 0) {}
        std::process::exit(0);
    });
}

/// The catalog this node serves before it adds its own store.
///
/// Testnet4: the demo stores and listings, with payment codes for testnet4,
/// accepting the testnet4 fixture moderator. Mainnet: empty; the demo data
/// is never served there.
pub fn load_catalog(network: NetworkMode) -> Result<Catalog, Error> {
    let catalog = match network {
        NetworkMode::Testnet4 => {
            let mut catalog = fixture_catalog();
            prepare_fixture_stores(&mut catalog, network)?;
            catalog
        }
        NetworkMode::Mainnet => Catalog {
            stores: Vec::new(),
            moderator: Moderator { name: String::new(), id: [0; 32] },
            listings: Vec::new(),
        },
    };
    catalog.verify().map_err(Error::Protocol)?;
    Ok(catalog)
}

/// The secret of `store` if it is a demo store, whose key is a public fixture.
pub(crate) fn demo_secret(store: &openbazaar_protocol::Store) -> Option<bitcoin::secp256k1::SecretKey> {
    let secret = fixture_store_keys(&store.name)?.store;
    let id = secret.x_only_public_key(&bitcoin::secp256k1::Secp256k1::new()).0.serialize();
    (id == *store.id.as_bytes()).then_some(secret)
}

/// The demo store secret for `store`, on testnet4 only.
fn fixture_secret(store: &openbazaar_protocol::Store, network: NetworkMode) -> Option<bitcoin::secp256k1::SecretKey> {
    (network == NetworkMode::Testnet4).then(|| demo_secret(store)).flatten()
}

/// Refuse a catalog whose store payment codes belong to another network,
/// or demo stores on mainnet.
fn check_catalog_network(catalog: &Catalog, network: NetworkMode) -> Result<(), Error> {
    if network == NetworkMode::Mainnet && catalog.stores.iter().any(|store| demo_secret(store).is_some()) {
        return Err(Error::Detail("The demo stores are testnet4 data; a mainnet node does not serve them.".into()));
    }
    for store in &catalog.stores {
        let pay = &store.direct_pay;
        network::offer_for(&pay.bolt12_offer, network).map_err(Error::Detail)?;
        network::code_for(&pay.silent_payment_code, network).map_err(Error::Detail)?;
    }
    Ok(())
}

/// Give each demo store a BOLT 12 offer and BIP 352 code for `network` and
/// the real escrow moderator id, and sign it again with its own key.
fn prepare_fixture_stores(catalog: &mut Catalog, network: NetworkMode) -> Result<(), Error> {
    let keys = fixture_keys();
    let record = escrow_fixture::moderator_record(network).map_err(Error::Detail)?;
    let moderator: [u8; 32] = record.signing_key[1..].try_into().expect("compressed key");
    catalog.moderator.id = moderator;
    let code = openbazaar_pay::SilentPaymentCode::from_keys(&keys.scan, &keys.spend, network.bitcoin())
        .map_err(|err| Error::Detail(err.to_string()))?
        .to_string();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    for store in &mut catalog.stores {
        let secret = fixture_store_keys(&store.name)
            .ok_or_else(|| Error::Detail(format!("{} is not a demo store", store.name)))?
            .store;
        let mut next = store.clone();
        let pubkey = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret);
        next.direct_pay = stores::direct_pay(pubkey, code.clone(), &store.name, network)?;
        if let Some(details) = next.details.as_mut() {
            details.moderators = vec![moderator];
        }
        *store = next.resign(&secret).map_err(Error::Protocol)?;
    }
    Ok(())
}

/// A listing takes escrow when it says so and its store's signed record
/// names the moderator this node uses.
pub(crate) fn accepts_moderator(catalog: &Catalog, listing: &Listing) -> bool {
    listing.moderated
        && !catalog.moderator.name.is_empty()
        && catalog
            .store_of(listing)
            .and_then(|store| store.details.as_ref())
            .is_some_and(|details| details.moderators.contains(&catalog.moderator.id))
}

pub(crate) fn find_listing<'a>(catalog: &'a Catalog, hash: &str) -> Option<&'a Listing> {
    let hash = hash.trim().trim_end_matches('/');
    catalog.listings.iter().find(|listing| {
        listing
            .content_hash()
            .ok()
            .is_some_and(|value| value.to_hex() == hash)
    })
}

pub(crate) fn listing_value(catalog: &Catalog, listing: &Listing) -> Result<Value, ProtocolError> {
    let mut value = json!({
        "hash": listing.content_hash()?.to_hex(),
        "storeId": listing.store_id.to_hex(),
        "title": listing.title,
        "category": listing.category,
        "priceSats": listing.price_sats,
        "shipFrom": listing.ship_from,
        "kind": listing.kind.as_str(),
        "description": listing.description,
        "seller": listing.seller,
        "stock": listing.stock,
        "listedOn": listing.listed_on,
        "product": listing.product,
        "shippingSats": listing.shipping_sats,
        "freeOverSats": listing.free_over_sats,
        "acceptsModerator": accepts_moderator(catalog, listing),
        "contentRating": rating_name(listing.content.map(|label| label.rating)),
        "contentTags": listing.content.map(|label| label.tags.names()).unwrap_or_default(),
        "record": listing.encode()?.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
    });
    if let Some(details) = &listing.details {
        value["listingId"] = json!(details.id.to_hex());
        value["images"] = json!(details.images.iter().map(|hash| escrow_mail::hex(hash)).collect::<Vec<_>>());
        value["price"] = details.price.as_ref().map_or(Value::Null, |price| {
            json!({ "currency": price.currency, "amount": price.amount, "shipping": price.shipping })
        });
    }
    Ok(value)
}

/// A record signed before labels existed is "unrated", not "general".
pub(crate) fn rating_name(rating: Option<ContentRating>) -> &'static str {
    rating.map_or("unrated", ContentRating::as_str)
}

pub fn catalog_value(catalog: &Catalog) -> Result<Value, ProtocolError> {
    let mut listings = Vec::with_capacity(catalog.listings.len());
    for listing in &catalog.listings {
        listings.push(listing_value(catalog, listing)?);
    }
    let mut stores = Vec::with_capacity(catalog.stores.len());
    for store in &catalog.stores {
        stores.push(stores::store_summary(catalog, store, None)?);
    }
    // `store` is the first store, for clients that predate `stores`.
    let first = catalog.stores.first().map(|store| {
        json!({
            "id": store.id.to_hex(),
            "name": store.name,
            "region": store.region,
            "contentRating": rating_name(store.rating),
        })
    });
    Ok(json!({
        "store": first,
        "stores": stores,
        "moderator": {
            "name": catalog.moderator.name,
            "id": catalog.moderator.id_hex(),
        },
        "listings": listings,
    }))
}

/// Shared secret for `POST /orders`, stored as `pairing.token` in the node data dir.
///
/// The first boot creates it. [`Node::fresh_pairing_token`] is how the process
/// learns that it should print the token once. The desktop app reads the file
/// and hands the token to its webview; a browser shop trades a one-time code
/// from the terminal for it at `POST /pair`.
fn load_pairing(dir: &Path) -> Result<(String, bool), Error> {
    let path = dir.join("pairing.token");
    if let Some(token) = read_pairing(&path)? {
        return Ok((token, false));
    }
    if path.exists() {
        fs::remove_file(&path).map_err(|err| Error::Detail(err.to_string()))?;
    }
    let token = generate_pairing_token();
    if write_pairing(&path, &token)? {
        return Ok((token, true));
    }
    // Another process created the file between the read and the write.
    match read_pairing(&path)? {
        Some(token) => Ok((token, false)),
        None => Err(Error::Detail(
            "the pairing token could not be read".to_string(),
        )),
    }
}

fn read_pairing(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let token = text.trim();
            if is_pairing_token(token) {
                Ok(Some(token.to_string()))
            } else {
                Ok(None)
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::Detail(err.to_string())),
    }
}

fn is_pairing_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn generate_pairing_token() -> String {
    let mut bytes = [0u8; 32];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns `true` when this process created the file.
fn write_pairing(path: &Path, token: &str) -> Result<bool, Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(Error::Detail(err.to_string())),
    };
    file.write_all(token.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|err| Error::Detail(err.to_string()))?;
    Ok(true)
}

pub fn catalog_json(catalog: &Catalog) -> Result<String, ProtocolError> {
    let value = catalog_value(catalog)?;
    Ok(serde_json::to_string_pretty(&value).expect("catalog json is finite"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn testnet4_serves_three_demo_stores_each_signing_its_own_listings() {
        use openbazaar_pay::{Bolt12Offer, SilentPaymentCode};
        let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let names: Vec<&str> = catalog.stores.iter().map(|store| store.name.as_str()).collect();
        assert_eq!(names, ["Marlow Goods", "Hale Paper", "North Kitchen"]);
        assert_eq!(catalog.moderator.name, "June Pell");
        let record = escrow_fixture::moderator_record(NetworkMode::Testnet4).unwrap();
        assert_eq!(catalog.moderator.id[..], record.signing_key[1..]);
        for store in &catalog.stores {
            store.verify().unwrap();
            assert_eq!(store.version(), 3);
            assert_eq!(store.details.as_ref().unwrap().moderators, [catalog.moderator.id]);
            assert!(network::offer_for(&store.direct_pay.bolt12_offer, NetworkMode::Testnet4).is_ok());
            assert!(network::offer_for(&store.direct_pay.bolt12_offer, NetworkMode::Mainnet).is_err());
            assert!(store.direct_pay.silent_payment_code.starts_with("tsp1"));
            assert!(Bolt12Offer::parse(&store.direct_pay.bolt12_offer).is_ok());
            assert!(SilentPaymentCode::parse(&store.direct_pay.silent_payment_code).is_ok());
            let own = catalog.listings.iter().filter(|listing| listing.store_id == store.id);
            assert!(own.clone().count() > 0, "{}", store.name);
            assert!(own.clone().all(|listing| listing.seller == store.name));
        }
        assert!(catalog.listings.iter().any(|listing| listing.kind.as_str() == "physical"));
        assert!(catalog.listings.iter().any(|listing| listing.kind.as_str() == "digital"));
        assert!(catalog.listings.iter().all(|listing| accepts_moderator(&catalog, listing) == listing.moderated));
    }

    #[test]
    fn mainnet_serves_no_demo_data() {
        let main = load_catalog(NetworkMode::Mainnet).unwrap();
        assert!(main.stores.is_empty() && main.listings.is_empty());
        assert!(main.moderator.name.is_empty());
        let test = load_catalog(NetworkMode::Testnet4).unwrap();
        assert!(check_catalog_network(&test, NetworkMode::Mainnet).is_err());
        assert!(check_catalog_network(&test, NetworkMode::Testnet4).is_ok());
    }

    #[test]
    fn shared_ui_catalog_matches_signed_records() {
        let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/mobile/src/catalog.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let committed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(committed, catalog_value(&catalog).unwrap());
    }

    #[test]
    fn catalog_headers_carry_the_signed_labels() {
        use openbazaar_protocol::{ContentLabel, ContentTags};
        let mut catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let value = catalog_value(&catalog).unwrap();
        assert_eq!(value["store"]["contentRating"], "general");
        assert!(value["listings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|listing| listing["contentRating"] == "general"
                && listing["contentTags"] == json!([])));

        catalog.listings[0].content = Some(ContentLabel::new(
            ContentRating::Sensitive,
            ContentTags::WEAPONS.with(ContentTags::VIOLENCE),
        ));
        catalog.listings[1].content = None;
        let value = catalog_value(&catalog).unwrap();
        assert_eq!(value["listings"][0]["contentRating"], "sensitive");
        assert_eq!(
            value["listings"][0]["contentTags"],
            json!(["violence", "weapons"])
        );
        assert_eq!(value["listings"][1]["contentRating"], "unrated");
        assert_eq!(value["listings"][1]["contentTags"], json!([]));
    }

    #[test]
    fn catalog_response_carries_the_json_body() {
        let json = catalog_json(&load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        let response = http_response("200 OK", "application/json", json.as_bytes());
        let text = String::from_utf8(response).unwrap();
        let (header, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(header.contains("200 OK"));
        assert!(header.contains(&format!("Content-Length: {}", json.len())));
        assert!(header.contains("Access-Control-Allow-Origin: http://127.0.0.1:4173"));
        assert_eq!(body, json);
        assert!(body.contains("Marlow Goods"));
        assert!(body.contains("Paperback novel"));
        assert!(body.contains("June Pell"));
    }
}
