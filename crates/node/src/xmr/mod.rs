//! Monero direct pay on this node, over [`openbazaar_xmr`].
//!
//! Every node wallet (`wallets/node`, `wallets/store-*`) has a Monero wallet
//! next to its bitcoin one, from the same phrase (`seed.rs`, path
//! `m/1984'/coin'/5'/0'`):
//!
//! | tier | file in `wallets/<name>/` | sealed with | holds |
//! |---|---|---|---|
//! | receive | `xmr-watch.obv` | vault key | private view key, public spend key |
//! | receive | `xmr-state.obv` | vault key | scanned height, outputs, next subaddress |
//! | spend | `spend.obv` (shared) | spend key | the phrase; the Monero spend key is derived from it for one signature |
//!
//! So a locked node still scans, issues subaddresses, and marks orders paid.
//! A wallet made before Monero existed gets its view keys from `seed.obv`
//! while that exists, or at the first unlock.
//!
//! Monero is off until `ob-node serve --xmr-daemon URL` (no default node, no
//! light-wallet server). Stores opt in one by one (`PUT /xmr/stores`). The
//! choice is kept on this node only: the signed store record does not carry
//! it yet (store record v3 is where it will go). Escrow is Bitcoin-only.

mod checkout;
mod http;
mod pay;
mod rate;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use openbazaar_xmr::{BackendStatus, SpendKey, ViewKeys, XmrChain, XmrNetwork, XmrWallet};
use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::wallet_keys::{self, SEED_FILE};
use crate::{Egress, Error, Node};

pub(crate) use checkout::{add_xmr_flags, catalog_response, rail_of};
pub(crate) use http::response;
pub(crate) use pay::XmrTerms;

const WATCH_FILE: &str = "xmr-watch.obv";
const STATE_FILE: &str = "xmr-state.obv";
const STORES_FILE: &str = "xmr-stores.obv";
/// Blocks fetched per request while syncing.
const SYNC_BLOCKS: u64 = 100;
/// Requests per wallet per round, so one wallet far behind does not starve the others.
const SYNC_STEPS: usize = 20;

/// One Monero wallet. `wallet` is `None` until its view keys are known.
pub(crate) struct Slot {
    dir: PathBuf,
    pub(crate) wallet: Option<XmrWallet>,
    pub(crate) last_sync: Option<u64>,
    pub(crate) error: Option<String>,
}

impl Slot {
    fn open(dir: &Path, vault: &Vault, network: NetworkMode) -> Result<Self, Error> {
        let keys = match vault.read_sealed(&dir.join(WATCH_FILE))? {
            Some(bytes) => Some(ViewKeys::decode(&bytes).map_err(Error::Detail)?),
            None if dir.join(SEED_FILE).exists() => {
                let seed = Seed::load_or_create(&dir.join(SEED_FILE), vault, network)?;
                Some(write_view_keys(dir, vault, &seed)?)
            }
            None => None,
        };
        let mut slot = Self { dir: dir.to_path_buf(), wallet: None, last_sync: None, error: None };
        if let Some(keys) = keys {
            slot.load(keys, vault, network)?;
        }
        Ok(slot)
    }

    fn load(&mut self, keys: ViewKeys, vault: &Vault, network: NetworkMode) -> Result<(), Error> {
        let wallet = match vault.read_sealed(&self.dir.join(STATE_FILE))? {
            Some(bytes) => XmrWallet::decode(&bytes, network.monero(), keys).map_err(Error::Detail)?,
            None => XmrWallet::new(network.monero(), keys),
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

fn write_view_keys(dir: &Path, vault: &Vault, seed: &Seed) -> Result<ViewKeys, Error> {
    let keys = SpendKey::from_seed_secret(&seed.monero_secret()).view_keys();
    vault.write_sealed(&dir.join(WATCH_FILE), &keys.encode())?;
    Ok(keys)
}

pub(crate) struct XmrNode {
    network: NetworkMode,
    vault: Vault,
    chain: OnceLock<Arc<dyn XmrChain>>,
    buyer: Mutex<Slot>,
    stores: BTreeMap<String, Mutex<Slot>>,
    accepts: Mutex<BTreeSet<String>>,
    accepts_path: PathBuf,
    pub(crate) rate: rate::XmrRate,
    jobs: pay::XmrJobs,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl XmrNode {
    /// `wallets` names each wallet directory under `dir/wallets`: the buyer's, then `(seller, dir name)`.
    pub(crate) fn open(
        dir: &Path,
        vault: &Vault,
        network: NetworkMode,
        sellers: &[(String, String)],
        egress: Egress,
    ) -> Result<Self, Error> {
        let wallets = dir.join("wallets");
        let buyer = Slot::open(&wallets.join("node"), vault, network)?;
        let mut stores = BTreeMap::new();
        for (seller, name) in sellers {
            stores.insert(seller.clone(), Mutex::new(Slot::open(&wallets.join(name), vault, network)?));
        }
        let accepts_path = dir.join(STORES_FILE);
        let accepts = match vault.read_sealed(&accepts_path)? {
            Some(bytes) => serde_json::from_slice::<Vec<String>>(&bytes)
                .map_err(|_| Error::Detail(format!("{STORES_FILE} could not be read")))?
                .into_iter()
                .collect(),
            None => BTreeSet::new(),
        };
        Ok(Self {
            network,
            vault: vault.clone(),
            chain: OnceLock::new(),
            buyer: Mutex::new(buyer),
            stores,
            accepts: Mutex::new(accepts),
            accepts_path,
            rate: rate::XmrRate::new(egress),
            jobs: pay::XmrJobs::new(),
        })
    }

    pub(crate) fn xmr_network(&self) -> XmrNetwork {
        self.network.monero()
    }

    pub(crate) fn chain(&self) -> Option<&Arc<dyn XmrChain>> {
        self.chain.get()
    }

    pub(crate) fn buyer(&self) -> MutexGuard<'_, Slot> {
        guard(&self.buyer)
    }

    pub(crate) fn store(&self, seller: &str) -> Option<MutexGuard<'_, Slot>> {
        self.stores.get(seller).map(guard)
    }

    pub(crate) fn accepts(&self, seller: &str) -> bool {
        guard(&self.accepts).contains(seller)
    }

    /// Opt a store in or out. Only stores whose wallets this node holds.
    pub(crate) fn set_accepts(&self, seller: &str, accept: bool) -> Result<(), String> {
        if !self.stores.contains_key(seller) {
            return Err("That seller has no wallet on this node.".into());
        }
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

    pub(crate) fn stores_value(&self) -> Value {
        let accepts = guard(&self.accepts);
        json!({
            "network": self.xmr_network().as_str(),
            "enabled": self.chain().is_some(),
            "stores": self.stores.keys().map(|seller| json!({ "seller": seller, "acceptsXmr": accepts.contains(seller) })).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn backend_value(&self) -> Value {
        let status = match self.chain() {
            Some(chain) => chain.status(),
            None => BackendStatus::off(self.xmr_network()),
        };
        let mut value = status.value();
        value["scannedHeight"] = json!(self.buyer().wallet.as_ref().and_then(XmrWallet::scanned));
        value
    }

    fn slots(&self) -> impl Iterator<Item = &Mutex<Slot>> {
        std::iter::once(&self.buyer).chain(self.stores.values())
    }
}

impl Node {
    /// Use `chain` for Monero. Without one, Monero checkout and pay are refused.
    pub fn set_xmr_chain(&self, chain: Arc<dyn XmrChain>) -> Result<(), String> {
        self.xmr.chain.set(chain).map_err(|_| "the Monero daemon is already set".to_string())
    }

    /// Derive view keys for wallets that had only a spend tier, while unlocked.
    fn xmr_fill_keys(&self) {
        let Ok(key) = self.lock.spend_key() else { return };
        for slot in self.xmr.slots() {
            let mut slot = guard(slot);
            if slot.wallet.is_some() {
                continue;
            }
            let loaded = wallet_keys::spend_seed(&slot.dir, &key, self.network)
                .and_then(|seed| write_view_keys(&slot.dir, &self.xmr.vault, &seed))
                .and_then(|keys| slot.load(keys, &self.xmr.vault, self.network));
            if let Err(err) = loaded {
                slot.error = Some(err.to_string());
            }
        }
    }

    /// One Monero round: sync every wallet, then update Monero orders. Returns how many orders changed.
    pub fn xmr_sync_once(&self) -> Result<usize, String> {
        let chain = self.xmr.chain().ok_or("Monero is off")?.clone();
        self.xmr_fill_keys();
        let mut failed = None;
        for slot in self.xmr.slots() {
            if let Err(err) = self.sync_slot(slot, chain.as_ref()) {
                failed = Some(err);
            }
        }
        let changed = self.xmr_update_orders();
        match failed {
            Some(err) if changed == 0 => Err(err),
            _ => Ok(changed),
        }
    }

    /// Sync a copy of the wallet without holding its lock, then merge it back.
    fn sync_slot(&self, slot: &Mutex<Slot>, chain: &dyn XmrChain) -> Result<(), String> {
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
                slot.save(&self.xmr.vault).map_err(|err| err.to_string())?;
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

    /// Sync and pay on one thread until `stop`: payments as they are queued, a sync every `every`.
    pub(crate) fn run_xmr(&self, every: Duration, stop: &AtomicBool) {
        if self.xmr.chain().is_none() {
            return;
        }
        let mut next_sync = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if Instant::now() >= next_sync {
                match self.xmr_sync_once() {
                    Ok(0) => {}
                    Ok(changed) => eprintln!("ob-node: {changed} Monero order status change(s)"),
                    Err(err) => eprintln!("ob-node: Monero: {err}"),
                }
                next_sync = Instant::now() + every;
            }
            self.run_xmr_pay_job(Duration::from_millis(250));
        }
    }
}
