//! A viewing-key wallet: which notes belong to it, at which height, which
//! order address each paid, and which of them have been spent.
//!
//! The UFVK finds and values incoming notes and computes their nullifiers,
//! so everything here works while the node's wallet is locked, including
//! seeing a spend made by another wallet restored from the same phrase.
//!
//! Ironwood is the pool: since NU6.3 every payment to an Orchard-protocol
//! receiver lands there, and only Ironwood notes pay an order. Orchard is
//! outbound only. Nothing new enters it, but notes made before NU6.3 can
//! still be spent out of it, so the wallet keeps reading it: a phrase
//! restored with old Orchard notes shows them, and shows them leave.
//!
//! Scanning is `zcash_client_backend::scanning::scan_block` over compact
//! blocks, with the Orchard-protocol keys for both pools. Spends are found
//! here by matching each block's nullifiers against the wallet's own notes;
//! the backend's spend tracking needs its database.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;

use orchard::keys::Scope;
use orchard::note::{NoteVersion, Nullifier, RandomSeed, Rho};
use orchard::note_encryption::{IronwoodDomain, OrchardDomain};
use orchard::value::NoteValue;
use orchard::Address;
use serde_json::{json, Value};
use subtle::CtOption;
use zcash_client_backend::data_api::BlockMetadata;
use zcash_client_backend::scanning::{scan_block, ScanningKey, ScanningKeyOps, ScanningKeys, SpendIdentifiers};
use zcash_client_backend::wallet::WalletOrchardOutput;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};

use crate::proto::CompactBlock;
use crate::trees::{self, Witness};
use crate::{ViewKey, ZecChain, ZecNetwork};

/// Confirmations before an order paid in Zcash counts as paid: about 12.5
/// minutes at NU7's 25-second blocks, the time ZIP 315's 10 confirmations
/// took at 75 seconds.
pub const CONFIRMATIONS: u64 = 30;
/// How far back the first sync starts: about 40 minutes.
const FIRST_SYNC_BACK: u64 = 100;
/// Block hashes kept to notice a reorg. Zebra treats 100 blocks as final.
const HASHES_KEPT: usize = 100;
/// ZIP 32 account 0, the only one this wallet has.
const ACCOUNT: u32 = 0;

/// The shielded pool a note is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    /// Outbound only since NU6.3: notes made before it can be spent, into
    /// Ironwood, and nothing new enters.
    Orchard,
    /// The default privacy pool since NU6.3, where every payment to an
    /// Orchard-protocol receiver lands, and the only pool that pays an order.
    Ironwood,
}

impl Pool {
    pub fn as_str(self) -> &'static str {
        match self {
            Pool::Orchard => "orchard",
            Pool::Ironwood => "ironwood",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "orchard" => Some(Pool::Orchard),
            "ironwood" => Some(Pool::Ironwood),
            _ => None,
        }
    }
}

/// Enough of a decrypted note to spend it again. Absent on a wallet file
/// written before spends existed; a later scan fills it in.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Material {
    recipient: [u8; 43],
    rho: [u8; 32],
    rseed: [u8; 32],
    /// 2 for Orchard, 3 for Ironwood.
    version: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Note {
    txid: [u8; 32],
    height: u64,
    pool: Pool,
    /// The order address it paid; `None` for change to the internal scope.
    index: Option<u32>,
    value: u64,
    nullifier: [u8; 32],
    /// The spending transaction and its block.
    spent: Option<([u8; 32], u64)>,
    material: Option<Material>,
    /// Which action in the transaction created it, within its pool.
    action: Option<u32>,
    /// Leaf position in this pool's commitment tree.
    position: Option<u64>,
    witness: Option<Witness>,
    /// A transaction we broadcast that spends this note, and the height it
    /// expires at. It leaves the balance until it is mined or expires.
    reserved: Option<([u8; 32], u64)>,
}

/// One confirmed Ironwood note the wallet can spend.
pub(crate) struct SpendNote {
    pub nullifier: [u8; 32],
    pub value: u64,
    pub note: orchard::Note,
    pub path: orchard::tree::MerklePath,
}

/// A scanned block: what the next one must chain onto, and the note
/// commitment tree sizes after it, which place the next block's notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mark {
    height: u64,
    hash: [u8; 32],
    sapling: u32,
    orchard: u32,
    ironwood: u32,
}

/// Cloned so a sync can run without holding the caller's lock; see [`ZecWallet::merge_sync`].
#[derive(Clone)]
pub struct ZecWallet {
    network: ZecNetwork,
    keys: ViewKey,
    /// The newest block scanned.
    scanned: Option<u64>,
    /// The chain tip at the last sync.
    tip: Option<u64>,
    marks: VecDeque<Mark>,
    /// Addresses `1..next_index` have been handed out; none is reused.
    next_index: u32,
    notes: Vec<Note>,
    /// Commitment frontiers after `trees_at`, when a scan has loaded them.
    orchard_tree: Option<Vec<u8>>,
    ironwood_tree: Option<Vec<u8>>,
    trees_at: Option<u64>,
    /// The next block a witness repair will replay. Not stored.
    repair_next: Option<u64>,
    /// Repair failed at this scanned height, so it waits for the next one.
    repair_failed_at: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    /// Unspent Ironwood, at least [`CONFIRMATIONS`] deep.
    pub confirmed: u64,
    /// Unspent Ironwood, not yet that deep.
    pub pending: u64,
    /// Unspent Orchard from before NU6.3. It can only move out, to Ironwood.
    pub orchard: u64,
}

/// What arrived at one order address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received {
    pub total: u64,
    /// Of `total`, the part at least [`CONFIRMATIONS`] deep.
    pub confirmed: u64,
    /// Confirmations of the newest note, if any arrived.
    pub confirmations: Option<u64>,
    pub txids: Vec<String>,
}

/// An unspent note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unspent {
    pub nullifier: [u8; 32],
    pub value: u64,
    pub pool: Pool,
    pub height: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncStep {
    pub tip: u64,
    pub scanned: u64,
    /// Notes found in this step.
    pub found: usize,
    /// Of the wallet's notes, those this step saw spent.
    pub spent: usize,
    /// Blocks dropped because the chain reorganized.
    pub rolled_back: u64,
}

impl SyncStep {
    pub fn caught_up(&self) -> bool {
        self.scanned >= self.tip
    }
}

type Keys = ScanningKeys<u32, (u32, Scope)>;

impl ZecWallet {
    pub fn new(network: ZecNetwork, keys: ViewKey) -> Self {
        Self {
            network,
            keys,
            scanned: None,
            tip: None,
            marks: VecDeque::new(),
            next_index: 1,
            notes: Vec::new(),
            orchard_tree: None,
            ironwood_tree: None,
            trees_at: None,
            repair_next: None,
            repair_failed_at: None,
        }
    }

    pub fn network(&self) -> ZecNetwork {
        self.network
    }

    pub fn keys(&self) -> &ViewKey {
        &self.keys
    }

    pub fn scanned(&self) -> Option<u64> {
        self.scanned
    }

    pub fn tip(&self) -> Option<u64> {
        self.tip
    }

    /// An address never handed out before. The caller saves the wallet
    /// before showing it, so a restart cannot hand it out again.
    pub fn issue_address(&mut self) -> (u32, String) {
        let index = self.next_index;
        self.next_index = index.checked_add(1).expect("fewer than 2^32 order addresses");
        (index, self.keys.address(index))
    }

    /// Scan up to `max_blocks` new blocks. Call until [`SyncStep::caught_up`].
    pub fn sync_step(&mut self, chain: &dyn ZecChain, max_blocks: u64) -> Result<SyncStep, String> {
        let tip = chain.tip()?;
        self.tip = Some(tip);
        self.release_expired();
        let rolled_back = self.check_reorg(chain)?;
        if self.needs_repair() {
            self.repair(chain, max_blocks)?;
            if self.needs_repair() {
                return Ok(SyncStep { tip, scanned: self.scanned.unwrap_or(tip), found: 0, spent: 0, rolled_back });
            }
        }
        let from = match self.scanned {
            Some(scanned) => scanned + 1,
            None => tip.saturating_sub(FIRST_SYNC_BACK),
        };
        let (mut found, mut spent) = (0, 0);
        if from <= tip {
            let to = tip.min(from + max_blocks.max(1) - 1);
            let blocks = chain.blocks(from, to)?;
            if blocks.len() as u64 != to - from + 1 {
                return Err("the Zcash server sent the wrong number of blocks".into());
            }
            let keys = self.scanning_keys();
            for (number, block) in (from..=to).zip(blocks) {
                let (block_found, block_spent) = self.scan(number, block, &keys, chain)?;
                found += block_found;
                spent += block_spent;
            }
        }
        self.tip = Some(tip);
        Ok(SyncStep { tip, scanned: self.scanned.unwrap_or(tip), found, spent, rolled_back })
    }

    fn scanning_keys(&self) -> Keys {
        type OrchardKey = Box<dyn ScanningKeyOps<OrchardDomain, u32, Nullifier> + Send + Sync>;
        type IronwoodKey = Box<dyn ScanningKeyOps<IronwoodDomain, u32, Nullifier> + Send + Sync>;
        let fvk = self.keys.orchard();
        let mut orchard: HashMap<(u32, Scope), OrchardKey> = HashMap::new();
        let mut ironwood: HashMap<(u32, Scope), IronwoodKey> = HashMap::new();
        for scope in [Scope::External, Scope::Internal] {
            let key = || ScanningKey::new(fvk.to_ivk(scope), Some(fvk.clone()), ACCOUNT, Some(scope));
            orchard.insert((ACCOUNT, scope), Box::new(key()));
            ironwood.insert((ACCOUNT, scope), Box::new(key()));
        }
        // No Sapling key: no address this wallet hands out has a Sapling receiver.
        ScanningKeys::new(HashMap::new(), orchard, ironwood)
    }

    /// Scan block `number`. Returns the notes found and the wallet notes it spends.
    fn scan(&mut self, number: u64, block: CompactBlock, keys: &Keys, chain: &dyn ZecChain) -> Result<(usize, usize), String> {
        if block.height != number {
            return Err(format!("the Zcash server sent block {} for {number}", block.height));
        }
        let hash = hash32(&block.hash).ok_or("the Zcash server sent a block hash that is not 32 bytes")?;
        let prior = self.marks.back().copied();
        if let Some(prior) = prior {
            if prior.height + 1 != number || block.prev_hash != prior.hash {
                return Err("the Zcash server sent blocks that do not chain".into());
            }
        }
        let revealed: Vec<([u8; 32], [u8; 32])> = block
            .vtx
            .iter()
            .filter_map(|tx| Some((hash32(&tx.txid)?, tx)))
            .flat_map(|(txid, tx)| {
                tx.actions.iter().chain(&tx.ironwood_actions).filter_map(move |action| Some((hash32(&action.nullifier)?, txid)))
            })
            .collect();
        let metadata = match prior {
            Some(mark) => Some(BlockMetadata::from_parts(
                height(mark.height)?,
                BlockHash(mark.hash),
                Some(mark.sapling),
                Some(mark.orchard),
                Some(mark.ironwood),
            )),
            None => None,
        };
        let trees = block.clone();
        let scanned = scan_block::<_, _, _, Infallible>(&self.network.params(), block, keys, &SpendIdentifiers::empty(), metadata.as_ref())
            .map_err(|err| format!("scanning block {number}: {err}"))?;
        let mut found = 0;
        for tx in scanned.transactions() {
            let txid = *tx.txid().as_ref();
            for output in tx.orchard_outputs().iter().chain(tx.ironwood_outputs()) {
                found += usize::from(self.record(txid, number, output));
            }
        }
        let mut spent = 0;
        for (nullifier, txid) in revealed {
            if let Some(note) = self.notes.iter_mut().find(|note| note.nullifier == nullifier && note.spent.is_none()) {
                note.spent = Some((txid, number));
                note.reserved = None;
                spent += 1;
            }
        }
        self.absorb(number, &trees, chain);
        let after = scanned.to_block_metadata();
        self.marks.push_back(Mark {
            height: number,
            hash,
            sapling: after.sapling_tree_size().unwrap_or(0),
            orchard: after.orchard_tree_size().unwrap_or(0),
            ironwood: after.ironwood_tree_size().unwrap_or(0),
        });
        while self.marks.len() > HASHES_KEPT {
            self.marks.pop_front();
        }
        self.scanned = Some(number);
        Ok((found, spent))
    }

    /// Keep one decrypted note. Returns whether it was new. A note already
    /// known from an older wallet file gets its spend material filled in.
    fn record(&mut self, txid: [u8; 32], height: u64, output: &WalletOrchardOutput<u32>) -> bool {
        let Some(nullifier) = output.nf().map(|nf| nf.to_bytes()) else { return false };
        let (note, pool) = output.note();
        let pool = match pool {
            orchard::ValuePool::Orchard => Pool::Orchard,
            orchard::ValuePool::Ironwood => Pool::Ironwood,
        };
        let material = material_of(note);
        if let Some(existing) = self.notes.iter_mut().find(|existing| existing.nullifier == nullifier) {
            if existing.material.is_none() {
                existing.material = Some(material);
                existing.action = u32::try_from(output.index()).ok();
            }
            return false;
        }
        // Orchard takes no new value from NU6.3 on, payment or change. A
        // server that sends a new Orchard note there is not following consensus.
        if pool == Pool::Orchard && self.ironwood_active(height) {
            return false;
        }
        let index = match output.recipient_key_scope() {
            Some(Scope::External) => self.keys.index_of(&note.recipient()),
            _ => None,
        };
        self.notes.push(Note {
            txid,
            height,
            pool,
            index,
            value: note.value().inner(),
            nullifier,
            spent: None,
            material: Some(material),
            action: u32::try_from(output.index()).ok(),
            position: None,
            witness: None,
            reserved: None,
        });
        true
    }

    /// Drop blocks no longer on the server's chain. Returns how many.
    fn check_reorg(&mut self, chain: &dyn ZecChain) -> Result<u64, String> {
        let Some(last) = self.marks.back().copied() else { return Ok(0) };
        if chain.block_hash(last.height)? == last.hash {
            return Ok(0);
        }
        let mut keep = None;
        for mark in self.marks.iter().rev().skip(1) {
            if chain.block_hash(mark.height)? == mark.hash {
                keep = Some(mark.height);
                break;
            }
        }
        let keep = keep.unwrap_or_else(|| self.marks.front().map_or(0, |mark| mark.height.saturating_sub(1)));
        self.marks.retain(|mark| mark.height <= keep);
        self.notes.retain(|note| note.height <= keep);
        for note in &mut self.notes {
            if note.spent.is_some_and(|(_, at)| at > keep) {
                note.spent = None;
            }
            // A witness includes every leaf after the note, so a dropped
            // block makes all of them stale. The next sync rebuilds them.
            note.witness = None;
            note.position = None;
        }
        self.orchard_tree = None;
        self.ironwood_tree = None;
        self.trees_at = None;
        self.repair_next = None;
        self.repair_failed_at = None;
        self.scanned = Some(keep);
        Ok(last.height - keep)
    }

    fn confirmations(&self, height: u64) -> u64 {
        self.tip.map_or(0, |tip| (tip + 1).saturating_sub(height))
    }

    fn ironwood_active(&self, height: u64) -> bool {
        u32::try_from(height).is_ok_and(|height| self.network.params().is_nu_active(NetworkUpgrade::Nu6_3, BlockHeight::from_u32(height)))
    }

    /// What arrived in Ironwood at order address `index`, spent or not. An
    /// Orchard note never pays an order.
    pub fn received(&self, index: u32) -> Received {
        let mut received = Received::default();
        for note in self.notes.iter().filter(|note| note.index == Some(index) && note.pool == Pool::Ironwood) {
            let confirmations = self.confirmations(note.height);
            received.total = received.total.saturating_add(note.value);
            if confirmations >= CONFIRMATIONS {
                received.confirmed = received.confirmed.saturating_add(note.value);
            }
            received.confirmations = Some(received.confirmations.map_or(confirmations, |seen| seen.min(confirmations)));
            let txid = txid_hex(&note.txid);
            if !received.txids.contains(&txid) {
                received.txids.push(txid);
            }
        }
        received
    }

    pub fn balance(&self) -> Balance {
        let mut balance = Balance::default();
        for note in self.notes.iter().filter(|note| self.available(note)) {
            if note.pool == Pool::Orchard {
                balance.orchard = balance.orchard.saturating_add(note.value);
            } else if self.confirmations(note.height) >= CONFIRMATIONS {
                balance.confirmed = balance.confirmed.saturating_add(note.value);
            } else {
                balance.pending = balance.pending.saturating_add(note.value);
            }
        }
        balance
    }

    /// Unspent notes from both pools, largest first. An Orchard note can only
    /// be spent into Ironwood.
    pub fn unspent(&self) -> Vec<Unspent> {
        let mut notes: Vec<Unspent> = self
            .notes
            .iter()
            .filter(|note| self.available(note))
            .map(|note| Unspent { nullifier: note.nullifier, value: note.value, pool: note.pool, height: note.height })
            .collect();
        notes.sort_by_key(|note| std::cmp::Reverse(note.value));
        notes
    }

    /// Take the scan from `synced`, a clone of this wallet that ran
    /// [`ZecWallet::sync_step`]. The viewing key decrypts every address, so,
    /// unlike Monero, an address handed out meanwhile does not make the scan
    /// stale; only the address counter is kept from `self`. Returns whether
    /// anything changed.
    pub fn merge_sync(&mut self, synced: ZecWallet) -> bool {
        let reserved: Vec<([u8; 32], [u8; 32], u64)> = self
            .notes
            .iter()
            .filter_map(|note| note.reserved.map(|(txid, expiry)| (note.nullifier, txid, expiry)))
            .collect();
        let changed = synced.scanned != self.scanned || synced.tip != self.tip || synced.notes != self.notes;
        self.scanned = synced.scanned;
        self.tip = synced.tip;
        self.marks = synced.marks;
        self.notes = synced.notes;
        self.orchard_tree = synced.orchard_tree;
        self.ironwood_tree = synced.ironwood_tree;
        self.trees_at = synced.trees_at;
        self.repair_next = synced.repair_next;
        self.repair_failed_at = synced.repair_failed_at;
        self.next_index = self.next_index.max(synced.next_index);
        for note in &mut self.notes {
            if note.spent.is_some() {
                note.reserved = None;
                continue;
            }
            if let Some((_, txid, expiry)) = reserved.iter().find(|(nullifier, _, _)| *nullifier == note.nullifier) {
                note.reserved = Some((*txid, *expiry));
            }
        }
        changed
    }

    /// Everything but the keys, which are stored apart in the receive tier.
    pub fn encode(&self) -> Vec<u8> {
        let value = json!({
            "version": 2,
            "network": self.network.as_str(),
            "address": self.keys.address(0),
            "scanned": self.scanned,
            "tip": self.tip,
            "marks": self.marks.iter().map(|mark| json!([mark.height, hex::encode(mark.hash), mark.sapling, mark.orchard, mark.ironwood])).collect::<Vec<_>>(),
            "nextIndex": self.next_index,
            "orchardTree": self.orchard_tree.as_ref().map(hex::encode),
            "ironwoodTree": self.ironwood_tree.as_ref().map(hex::encode),
            "treesAt": self.trees_at,
            "notes": self.notes.iter().map(|note| json!({
                "txid": hex::encode(note.txid),
                "height": note.height,
                "pool": note.pool.as_str(),
                "index": note.index,
                "value": note.value,
                "nullifier": hex::encode(note.nullifier),
                "spentBy": note.spent.map(|(txid, _)| hex::encode(txid)),
                "spentAt": note.spent.map(|(_, at)| at),
                "recipient": note.material.as_ref().map(|item| hex::encode(item.recipient)),
                "rho": note.material.as_ref().map(|item| hex::encode(item.rho)),
                "rseed": note.material.as_ref().map(|item| hex::encode(item.rseed)),
                "noteVersion": note.material.as_ref().map(|item| item.version),
                "action": note.action,
                "position": note.position,
                "witness": note.witness.as_ref().map(Witness::to_json),
                "reservedBy": note.reserved.map(|(txid, _)| hex::encode(txid)),
                "reservedUntil": note.reserved.map(|(_, expiry)| expiry),
            })).collect::<Vec<_>>(),
        });
        serde_json::to_vec(&value).expect("the wallet is plain json")
    }

    pub fn decode(bytes: &[u8], network: ZecNetwork, keys: ViewKey) -> Result<Self, String> {
        let bad = || "the Zcash wallet file could not be read".to_string();
        let value: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
        let mut wallet = Self::new(network, keys);
        let version = value["version"].as_u64();
        if (version != Some(1) && version != Some(2))
            || value["network"].as_str() != Some(network.as_str())
            || value["address"].as_str() != Some(wallet.keys.address(0).as_str())
        {
            return Err("the Zcash wallet file belongs to another wallet or network".into());
        }
        wallet.scanned = value["scanned"].as_u64();
        wallet.tip = value["tip"].as_u64();
        wallet.next_index = value["nextIndex"].as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n >= 1).ok_or_else(bad)?;
        let size = |value: &Value| value.as_u64().and_then(|n| u32::try_from(n).ok());
        for entry in value["marks"].as_array().ok_or_else(bad)? {
            wallet.marks.push_back(Mark {
                height: entry[0].as_u64().ok_or_else(bad)?,
                hash: hash32_hex(&entry[1]).ok_or_else(bad)?,
                sapling: size(&entry[2]).ok_or_else(bad)?,
                orchard: size(&entry[3]).ok_or_else(bad)?,
                ironwood: size(&entry[4]).ok_or_else(bad)?,
            });
        }
        for entry in value["notes"].as_array().ok_or_else(bad)? {
            let index = match &entry["index"] {
                Value::Null => None,
                index => Some(size(index).ok_or_else(bad)?),
            };
            let spent = match (&entry["spentBy"], entry["spentAt"].as_u64()) {
                (Value::Null, _) => None,
                (txid, Some(at)) => Some((hash32_hex(txid).ok_or_else(bad)?, at)),
                _ => return Err(bad()),
            };
            let material = match (hash32_hex(&entry["rho"]), hash32_hex(&entry["rseed"]), entry["noteVersion"].as_u64()) {
                (Some(rho), Some(rseed), Some(version)) => {
                    let recipient = hex::decode(entry["recipient"].as_str().unwrap_or_default()).ok().and_then(|bytes| bytes.try_into().ok());
                    recipient.map(|recipient| Material { recipient, rho, rseed, version: version as u8 })
                }
                _ => None,
            };
            let reserved = match entry["reservedUntil"].as_u64() {
                Some(expiry) => Some((hash32_hex(&entry["reservedBy"]).ok_or_else(bad)?, expiry)),
                None => None,
            };
            wallet.notes.push(Note {
                txid: hash32_hex(&entry["txid"]).ok_or_else(bad)?,
                height: entry["height"].as_u64().ok_or_else(bad)?,
                pool: entry["pool"].as_str().and_then(Pool::parse).ok_or_else(bad)?,
                index,
                value: entry["value"].as_u64().ok_or_else(bad)?,
                nullifier: hash32_hex(&entry["nullifier"]).ok_or_else(bad)?,
                spent,
                material,
                action: entry["action"].as_u64().and_then(|n| u32::try_from(n).ok()),
                position: entry["position"].as_u64(),
                witness: entry.get("witness").and_then(Witness::from_json),
                reserved,
            });
        }
        if version == Some(2) {
            wallet.orchard_tree = hex_tree(&value["orchardTree"]);
            wallet.ironwood_tree = hex_tree(&value["ironwoodTree"]);
            wallet.trees_at = value["treesAt"].as_u64();
        }
        Ok(wallet)
    }

    /// The address a restored wallet shows first. Order addresses start at 1.
    pub fn receive_address(&self) -> String {
        self.keys.address(0)
    }

    pub(crate) fn trees_height(&self) -> Option<u64> {
        self.trees_at
    }

    /// Hold `nullifiers` out of the balance until `expiry` or until the spend is mined.
    pub fn reserve(&mut self, nullifiers: &[[u8; 32]], txid: [u8; 32], expiry: u64) {
        for note in &mut self.notes {
            if nullifiers.contains(&note.nullifier) && note.spent.is_none() {
                note.reserved = Some((txid, expiry));
            }
        }
    }

    pub(crate) fn ironwood_anchor(&self) -> Option<orchard::Anchor> {
        if self.trees_at != self.scanned {
            return None;
        }
        let tree = trees::read_tree(self.ironwood_tree.as_ref()?).ok()?;
        Some(orchard::Anchor::from(tree.root()))
    }

    /// Confirmed, witnessed Ironwood notes, largest first.
    pub(crate) fn ironwood_spendable(&self) -> Vec<SpendNote> {
        let mut notes: Vec<&Note> = self
            .notes
            .iter()
            .filter(|note| note.pool == Pool::Ironwood && self.available(note) && self.confirmations(note.height) >= CONFIRMATIONS)
            .filter(|note| note.material.is_some() && note.witness.is_some())
            .collect();
        notes.sort_by_key(|note| std::cmp::Reverse(note.value));
        notes.iter().filter_map(|note| self.spend_note(note)).collect()
    }

    pub(crate) fn ironwood_inputs(&self, nullifiers: &[[u8; 32]]) -> Result<Vec<SpendNote>, String> {
        nullifiers
            .iter()
            .map(|nullifier| {
                let note = self.notes.iter().find(|note| note.nullifier == *nullifier).ok_or("a note chosen for this payment is gone")?;
                self.spend_note(note).ok_or_else(|| "a note chosen for this payment cannot be spent yet".to_string())
            })
            .collect()
    }

    fn spend_note(&self, note: &Note) -> Option<SpendNote> {
        let plain = note_from(note.material.as_ref()?, note.value)?;
        let path = note.witness.as_ref()?.path().ok()?;
        Some(SpendNote { nullifier: note.nullifier, value: note.value, note: plain, path })
    }

    fn available(&self, note: &Note) -> bool {
        if note.spent.is_some() {
            return false;
        }
        match note.reserved {
            Some((_, expiry)) => self.tip.is_some_and(|tip| tip >= expiry),
            None => true,
        }
    }

    fn release_expired(&mut self) {
        let Some(tip) = self.tip else { return };
        for note in &mut self.notes {
            if note.reserved.is_some_and(|(_, expiry)| tip >= expiry) {
                note.reserved = None;
            }
        }
    }

    fn needs_repair(&self) -> bool {
        self.repair_failed_at != self.scanned && self.notes.iter().any(|note| note.spent.is_none() && (note.material.is_none() || note.witness.is_none()))
    }

    /// Replay blocks that hold notes without a witness, up to `max_blocks`.
    fn repair(&mut self, chain: &dyn ZecChain, max_blocks: u64) -> Result<(), String> {
        let Some(start) = self.notes.iter().filter(|note| note.spent.is_none() && (note.material.is_none() || note.witness.is_none())).map(|note| note.height).min() else {
            return Ok(());
        };
        if self.repair_next.is_none() {
            self.clear_witnesses();
        }
        let Some(scanned) = self.scanned else { return Ok(()) };
        let from = self.repair_next.unwrap_or(start).min(scanned);
        if self.trees_at != Some(from.saturating_sub(1)) && !self.load_trees(chain, from.saturating_sub(1), None, None) {
            self.repair_failed_at = self.scanned;
            self.repair_next = None;
            return Ok(());
        }
        let to = scanned.min(from.saturating_add(max_blocks.max(1) - 1));
        let blocks = chain.blocks(from, to)?;
        if blocks.len() as u64 != to - from + 1 {
            self.repair_failed_at = self.scanned;
            return Ok(());
        }
        let keys = self.scanning_keys();
        for (number, block) in (from..=to).zip(blocks) {
            self.observe(number, block, &keys, chain)?;
        }
        self.repair_next = Some(to + 1);
        if to == scanned {
            self.repair_next = None;
        }
        Ok(())
    }

    /// Decrypt `block` and extend witnesses. Does not move the scanned height.
    fn observe(&mut self, number: u64, block: CompactBlock, keys: &Keys, chain: &dyn ZecChain) -> Result<(), String> {
        if block.height != number {
            return Ok(());
        }
        let trees = block.clone();
        let scanned = scan_block::<_, _, _, Infallible>(&self.network.params(), block, keys, &SpendIdentifiers::empty(), None)
            .map_err(|err| format!("scanning block {number}: {err}"))?;
        for tx in scanned.transactions() {
            let txid = *tx.txid().as_ref();
            for output in tx.orchard_outputs().iter().chain(tx.ironwood_outputs()) {
                self.record(txid, number, output);
            }
        }
        self.absorb(number, &trees, chain);
        Ok(())
    }

    /// Append this block's commitments and witness any note of ours in it.
    fn absorb(&mut self, number: u64, block: &CompactBlock, chain: &dyn ZecChain) {
        let prior = self.marks.iter().rev().find(|mark| mark.height + 1 == number).copied();
        let orchard_prior = prior_size(prior.map(|mark| mark.orchard), block, Pool::Orchard);
        let ironwood_prior = prior_size(prior.map(|mark| mark.ironwood), block, Pool::Ironwood);
        if self.trees_at != Some(number.saturating_sub(1)) && !self.load_trees(chain, number.saturating_sub(1), orchard_prior, ironwood_prior) {
            self.clear_witnesses();
            self.repair_failed_at = self.scanned;
            return;
        }
        if !self.append_pool(Pool::Orchard, block, orchard_prior) || !self.append_pool(Pool::Ironwood, block, ironwood_prior) {
            self.clear_witnesses();
            self.repair_failed_at = self.scanned;
            return;
        }
        self.trees_at = Some(number);
    }

    fn clear_witnesses(&mut self) {
        for note in &mut self.notes {
            note.witness = None;
            note.position = None;
        }
        self.orchard_tree = None;
        self.ironwood_tree = None;
        self.trees_at = None;
    }

    fn load_trees(&mut self, chain: &dyn ZecChain, height: u64, orchard_size: Option<u32>, ironwood_size: Option<u32>) -> bool {
        if orchard_size == Some(0) && ironwood_size == Some(0) {
            self.orchard_tree = Some(trees::write_tree(&trees::Tree::empty()));
            self.ironwood_tree = Some(trees::write_tree(&trees::Tree::empty()));
            self.trees_at = Some(height);
            return true;
        }
        let Ok(state) = chain.tree_state(height) else { return false };
        let Ok(orchard) = trees::read_tree(&state.orchard) else { return false };
        let Ok(ironwood) = trees::read_tree(&state.ironwood) else { return false };
        if orchard_size.is_some_and(|size| orchard.size() != size as usize) || ironwood_size.is_some_and(|size| ironwood.size() != size as usize) {
            return false;
        }
        self.orchard_tree = Some(state.orchard);
        self.ironwood_tree = Some(state.ironwood);
        self.trees_at = Some(height);
        true
    }

    fn append_pool(&mut self, pool: Pool, block: &CompactBlock, prior: Option<u32>) -> bool {
        let Some(prior) = prior else { return false };
        let stored = match pool {
            Pool::Orchard => self.orchard_tree.clone(),
            Pool::Ironwood => self.ironwood_tree.clone(),
        };
        let Some(stored) = stored else { return false };
        let Ok(mut tree) = trees::read_tree(&stored) else { return false };
        if tree.size() != prior as usize {
            return false;
        }
        let mut position = u64::from(prior);
        for tx in &block.vtx {
            let Some(txid) = hash32(&tx.txid) else { return false };
            for (index, action) in trees::actions_of(tx, pool).iter().enumerate() {
                let Some(node) = trees::node_from_cmx(&action.cmx) else { return false };
                if tree.append(node).is_err() {
                    return false;
                }
                let action_index = u32::try_from(index).ok();
                for note in self.notes.iter_mut().filter(|note| note.pool == pool) {
                    if note.witness.is_some() {
                        let failed = note.witness.as_mut().is_some_and(|witness| witness.append(node).is_err());
                        if failed {
                            return false;
                        }
                    } else if note.txid == txid && note.action == action_index {
                        note.position = Some(position);
                        note.witness = Witness::watching(&tree);
                    }
                }
                position += 1;
            }
        }
        let written = trees::write_tree(&tree);
        match pool {
            Pool::Orchard => self.orchard_tree = Some(written),
            Pool::Ironwood => self.ironwood_tree = Some(written),
        }
        true
    }
}

fn height(number: u64) -> Result<BlockHeight, String> {
    u32::try_from(number).map(BlockHeight::from_u32).map_err(|_| format!("block {number} is out of range"))
}

fn hash32(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}

fn hash32_hex(value: &Value) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(value.as_str()?, &mut out).ok()?;
    Some(out)
}

/// A transaction id as block explorers and wallets show it: byte-reversed hex.
fn txid_hex(txid: &[u8; 32]) -> String {
    let mut reversed = *txid;
    reversed.reverse();
    hex::encode(reversed)
}

fn material_of(note: &orchard::Note) -> Material {
    Material {
        recipient: note.recipient().to_raw_address_bytes(),
        rho: note.rho().to_bytes(),
        rseed: *note.rseed().as_bytes(),
        version: match note.version() {
            NoteVersion::V2 => 2,
            NoteVersion::V3 => 3,
        },
    }
}

fn note_from(material: &Material, value: u64) -> Option<orchard::Note> {
    let recipient = option(Address::from_raw_address_bytes(&material.recipient))?;
    let rho = option(Rho::from_bytes(&material.rho))?;
    let rseed = option(RandomSeed::from_bytes(material.rseed, &rho))?;
    let version = match material.version {
        2 => NoteVersion::V2,
        3 => NoteVersion::V3,
        _ => return None,
    };
    option(orchard::Note::from_parts(recipient, NoteValue::from_raw(value), rho, rseed, version))
}

fn prior_size(mark: Option<u32>, block: &CompactBlock, pool: Pool) -> Option<u32> {
    if let Some(size) = mark {
        return Some(size);
    }
    let after = block.chain_metadata.as_ref()?;
    let count: u32 = block.vtx.iter().map(|tx| trees::actions_of(tx, pool).len() as u32).sum();
    let after = match pool {
        Pool::Orchard => after.orchard_commitment_tree_size,
        Pool::Ironwood => after.ironwood_commitment_tree_size,
    };
    after.checked_sub(count)
}

fn hex_tree(value: &Value) -> Option<Vec<u8>> {
    hex::decode(value.as_str()?).ok()
}

fn option<T>(value: CtOption<T>) -> Option<T> {
    value.into_option()
}
