//! Zcash direct pay on this node, over [`openbazaar_zec`].
//!
//! Each demo store's wallet (`wallets/store-*`) has a Zcash wallet next to
//! its bitcoin one, from the same phrase (ZIP 32 account 0 of the phrase's
//! BIP 39 seed; see `seed.rs`):
//!
//! | tier | file in `wallets/<name>/` | sealed with | holds |
//! |---|---|---|---|
//! | receive | `zec-watch.obv` | vault key | the UFVK |
//! | receive | `zec-state.obv` | vault key | scanned height, block marks, notes, next order address |
//! | spend | `spend.obv` (shared) | spend key | the phrase |
//!
//! So a locked node still scans, hands out order addresses, and marks orders
//! paid. A wallet made before Zcash existed gets its viewing key from
//! `seed.obv` while that exists, or at the first unlock.
//!
//! Zcash is off until a chain source is set: `--zec-indexer` names a Zaino or
//! lightwalletd ([`openbazaar_zec::IndexerChain`], set by `main` through
//! [`Node::set_zec_chain`]), and tests set the mock. Demo stores opt in one by one
//! (`PUT /zec/stores`), which signs the store's v3 record again with bit 2
//! set; checkout and the catalog read the signed flag only. `zec-stores.obv`
//! keeps the choice for the next boot. The node's own store does not take
//! Zcash yet: its record goes to other nodes, a release from before bit 2
//! refuses the whole record, and a sale from another node carries bitcoin
//! terms only (phase 2). Ironwood is the pool that pays; escrow is
//! Bitcoin-only.

mod checkout;
mod http;
mod rate;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{Catalog, Store};
use openbazaar_zec::{BackendStatus, ViewKey, ZecChain, ZecNetwork, ZecWallet};
use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::wallet_keys::{self, SEED_FILE};
use crate::{Egress, Error, Node};

pub(crate) use checkout::{add_zec_flags, rail_of};
pub(crate) use http::response;

const WATCH_FILE: &str = "zec-watch.obv";
const STATE_FILE: &str = "zec-state.obv";
const STORES_FILE: &str = "zec-stores.obv";
/// Compact blocks fetched per request while syncing.
const SYNC_BLOCKS: u64 = 100;
/// Requests per wallet per round, so one wallet far behind does not starve the others.
const SYNC_STEPS: usize = 20;
/// How often the sync thread looks at its stop flag.
const TICK: Duration = Duration::from_millis(250);

/// Why the own store cannot take Zcash yet.
pub(crate) const OWN_STORE_LATER: &str = "Your own store takes Zcash once sales from other nodes carry Zcash terms. Demo stores on this node take it now.";

/// One Zcash wallet. `wallet` is `None` until its viewing key is known.
pub(crate) struct Slot {
    dir: PathBuf,
    pub(crate) wallet: Option<ZecWallet>,
    pub(crate) last_sync: Option<u64>,
    pub(crate) error: Option<String>,
}

impl Slot {
    fn open(dir: &Path, vault: &Vault, network: NetworkMode) -> Result<Self, Error> {
        let keys = match vault.read_sealed(&dir.join(WATCH_FILE))? {
            Some(bytes) => Some(ViewKey::decode(&bytes, network.zcash()).map_err(Error::Detail)?),
            None if dir.join(SEED_FILE).exists() => {
                let seed = Seed::load_or_create(&dir.join(SEED_FILE), vault, network)?;
                Some(write_view_key(dir, vault, &seed)?)
            }
            None => None,
        };
        let mut slot = Self { dir: dir.to_path_buf(), wallet: None, last_sync: None, error: None };
        if let Some(keys) = keys {
            slot.load(keys, vault, network)?;
        }
        Ok(slot)
    }

    fn load(&mut self, keys: ViewKey, vault: &Vault, network: NetworkMode) -> Result<(), Error> {
        let wallet = match vault.read_sealed(&self.dir.join(STATE_FILE))? {
            Some(bytes) => ZecWallet::decode(&bytes, network.zcash(), keys).map_err(Error::Detail)?,
            None => ZecWallet::new(network.zcash(), keys),
        };
        self.wallet = Some(wallet);
        Ok(())
    }

    pub(crate) fn save(&self, vault: &Vault) -> Result<(), Error> {
        match &self.wallet {
            Some(wallet) => vault.write_sealed(&self.dir.join(STATE_FILE), &wallet.encode()),
            None => Ok(()),
        }
    }
}

fn write_view_key(dir: &Path, vault: &Vault, seed: &Seed) -> Result<ViewKey, Error> {
    let key = seed.zcash_view_key()?;
    vault.write_sealed(&dir.join(WATCH_FILE), &key.encode())?;
    Ok(key)
}

pub(crate) struct ZecNode {
    network: NetworkMode,
    vault: Vault,
    chain: OnceLock<Arc<dyn ZecChain>>,
    stores: BTreeMap<String, Mutex<Slot>>,
    accepts: Mutex<BTreeSet<String>>,
    accepts_path: PathBuf,
    pub(crate) rate: rate::ZecRate,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl ZecNode {
    /// `sellers` names each demo store's wallet directory under `dir/wallets`, as `(seller, dir name)`.
    pub(crate) fn open(dir: &Path, vault: &Vault, network: NetworkMode, sellers: &[(String, String)], egress: Egress) -> Result<Self, Error> {
        let wallets = dir.join("wallets");
        let mut stores = BTreeMap::new();
        for (seller, name) in sellers {
            stores.insert(seller.clone(), Mutex::new(Slot::open(&wallets.join(name), vault, network)?));
        }
        let accepts_path = dir.join(STORES_FILE);
        let accepts = read_accepts(&accepts_path, vault)?;
        Ok(Self {
            network,
            vault: vault.clone(),
            chain: OnceLock::new(),
            stores,
            accepts: Mutex::new(accepts),
            accepts_path,
            rate: rate::ZecRate::new(egress),
        })
    }

    pub(crate) fn zec_network(&self) -> ZecNetwork {
        self.network.zcash()
    }

    pub(crate) fn chain(&self) -> Option<&Arc<dyn ZecChain>> {
        self.chain.get()
    }

    pub(crate) fn store(&self, seller: &str) -> Option<MutexGuard<'_, Slot>> {
        self.stores.get(seller).map(guard)
    }

    /// Remember a demo store's choice for the next boot.
    fn save_accepts(&self, seller: &str, accept: bool) -> Result<(), String> {
        let mut accepts = guard(&self.accepts);
        if accept {
            accepts.insert(seller.to_string());
        } else {
            accepts.remove(seller);
        }
        let list: Vec<&String> = accepts.iter().collect();
        let bytes = serde_json::to_vec(&list).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.accepts_path, &bytes).map_err(|err| err.to_string())
    }

    /// The `zecBackend` field of `GET /network`: what the indexer is and
    /// learns, how far the wallets have scanned, and the last sync error.
    pub(crate) fn backend_value(&self) -> Value {
        let mut value = match self.chain() {
            Some(chain) => chain.status().value(),
            None => BackendStatus::off(self.zec_network()).value(),
        };
        let scanned = self.stores.values().filter_map(|slot| guard(slot).wallet.as_ref().and_then(ZecWallet::scanned)).min();
        let error = self.stores.values().find_map(|slot| guard(slot).error.clone());
        value["enabled"] = json!(self.chain().is_some());
        value["pool"] = json!("ironwood");
        value["scannedHeight"] = json!(scanned);
        if value["error"].is_null() {
            value["error"] = json!(error);
        }
        value
    }
}

fn read_accepts(path: &Path, vault: &Vault) -> Result<BTreeSet<String>, Error> {
    match vault.read_sealed(path)? {
        Some(bytes) => Ok(serde_json::from_slice::<Vec<String>>(&bytes)
            .map_err(|_| Error::Detail(format!("{STORES_FILE} could not be read")))?
            .into_iter()
            .collect()),
        None => Ok(BTreeSet::new()),
    }
}

/// Sign each demo store that opted in to Zcash again with `acceptsZec`.
pub(crate) fn apply_signed_accepts(dir: &Path, vault: &Vault, catalog: &mut Catalog, network: NetworkMode) -> Result<(), Error> {
    let accepts = read_accepts(&dir.join(STORES_FILE), vault)?;
    for store in &mut catalog.stores {
        if !accepts.contains(&store.name) {
            continue;
        }
        let Some(secret) = crate::fixture_secret(store, network) else { continue };
        *store = with_zec(store, true, &secret).map_err(Error::Protocol)?;
    }
    Ok(())
}

/// `store` signed again with `accept` as its Zcash flag.
fn with_zec(store: &Store, accept: bool, secret: &SecretKey) -> Result<Store, openbazaar_protocol::Error> {
    let mut next = store.clone();
    if let Some(details) = next.details.as_mut() {
        details.accepts_zec = accept;
    }
    next.resign(secret)
}

/// Why a store's Zcash choice could not change.
pub(crate) enum AcceptError {
    Unknown(String),
    OwnStore,
    Failed(String),
}

impl Node {
    /// What the store named `seller` signed about Zcash.
    pub(crate) fn accepts_zec(&self, seller: &str) -> bool {
        self.catalog()
            .stores
            .iter()
            .find(|store| store.name == seller)
            .and_then(|store| store.details.as_ref())
            .is_some_and(|details| details.accepts_zec)
    }

    /// Opt a demo store in or out by signing its record again.
    pub(crate) fn set_accepts_zec(&self, seller: &str, accept: bool) -> Result<(), AcceptError> {
        let store = self.seller_store(seller).map_err(AcceptError::Unknown)?;
        if store.id == self.own_store {
            return Err(AcceptError::OwnStore);
        }
        if !self.zec.stores.contains_key(seller) {
            return Err(AcceptError::Unknown("That seller has no wallet on this node.".into()));
        }
        if store.details.is_none() {
            return Err(AcceptError::Unknown("That store's record cannot carry a Zcash choice.".into()));
        }
        let secret = self.store_secret(&store.id).ok_or_else(|| AcceptError::Unknown("This node does not hold that store's key.".into()))?;
        let signed = with_zec(&store, accept, &secret).map_err(|err| AcceptError::Failed(err.to_string()))?;
        self.zec.save_accepts(seller, accept).map_err(AcceptError::Failed)?;
        let mut catalog = self.catalog_mut();
        if let Some(old) = catalog.stores.iter_mut().find(|old| old.id == signed.id) {
            *old = signed;
        }
        Ok(())
    }

    pub(crate) fn zec_stores_value(&self) -> Value {
        json!({
            "network": self.zec.zec_network().as_str(),
            "enabled": self.zec.chain().is_some(),
            "stores": self.zec.stores.keys().map(|seller| json!({ "seller": seller, "acceptsZec": self.accepts_zec(seller) })).collect::<Vec<_>>(),
        })
    }

    /// Use `chain` for Zcash. Without one, Zcash checkout is refused.
    pub fn set_zec_chain(&self, chain: Arc<dyn ZecChain>) -> Result<(), String> {
        self.zec.chain.set(chain).map_err(|_| "the Zcash chain source is already set".to_string())
    }

    /// Derive viewing keys for wallets that had only a spend tier, while unlocked.
    fn zec_fill_keys(&self) {
        let Ok(key) = self.lock.spend_key() else { return };
        for slot in self.zec.stores.values() {
            let mut slot = guard(slot);
            if slot.wallet.is_some() {
                continue;
            }
            let loaded = wallet_keys::spend_seed(&slot.dir, &key, self.network)
                .and_then(|seed| write_view_key(&slot.dir, &self.zec.vault, &seed))
                .and_then(|keys| slot.load(keys, &self.zec.vault, self.network));
            if let Err(err) = loaded {
                slot.error = Some(err.to_string());
            }
        }
    }

    /// One Zcash round: sync every wallet, then update Zcash orders. Returns how many orders changed.
    pub fn zec_sync_once(&self) -> Result<usize, String> {
        let chain = self.zec.chain().ok_or("Zcash is off")?.clone();
        self.zec_fill_keys();
        let mut failed = None;
        for slot in self.zec.stores.values() {
            if let Err(err) = self.sync_zec_slot(slot, chain.as_ref()) {
                failed = Some(err);
            }
        }
        let changed = self.zec_update_orders();
        match failed {
            Some(err) if changed == 0 => Err(err),
            _ => Ok(changed),
        }
    }

    /// Sync a copy of the wallet without holding its lock, then merge it back.
    /// An order address handed out meanwhile is found by the copy too: the
    /// viewing key decrypts every address.
    fn sync_zec_slot(&self, slot: &Mutex<Slot>, chain: &dyn ZecChain) -> Result<(), String> {
        let Some(mut copy) = guard(slot).wallet.clone() else { return Ok(()) };
        let mut result = Ok(());
        for _ in 0..SYNC_STEPS {
            match copy.sync_step(chain, SYNC_BLOCKS) {
                Ok(step) if step.caught_up() => break,
                Ok(_) => {}
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        let mut slot = guard(slot);
        if let Some(wallet) = slot.wallet.as_mut() {
            if wallet.merge_sync(copy) {
                slot.save(&self.zec.vault).map_err(|err| err.to_string())?;
            }
        }
        match &result {
            Ok(()) => {
                slot.last_sync = Some(now_secs());
                slot.error = None;
            }
            Err(err) => slot.error = Some(err.clone()),
        }
        result
    }

    /// Sync every `every` on this thread until `stop`.
    pub(crate) fn run_zec(&self, every: Duration, stop: &AtomicBool) {
        if self.zec.chain().is_none() {
            return;
        }
        let mut next_sync = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if Instant::now() >= next_sync {
                match self.zec_sync_once() {
                    Ok(0) => {}
                    Ok(changed) => eprintln!("ob-node: {changed} Zcash order status change(s)"),
                    Err(err) => eprintln!("ob-node: Zcash: {err}"),
                }
                next_sync = Instant::now() + every;
            }
            std::thread::sleep(TICK);
        }
    }
}
