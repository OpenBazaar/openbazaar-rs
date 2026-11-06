//! A view-key wallet: which outputs belong to it, at which height, and which
//! of them this node has spent.
//!
//! The private view key finds and values incoming outputs, so everything
//! here works while the node's wallet is locked. Spending needs the spend key
//! and happens in `send.rs`.
//!
//! Without the spend key the wallet cannot compute key images, so it cannot
//! see a spend made by another wallet restored from the same seed. It only
//! knows about spends this node signed. The node never shows its Monero
//! spend key, so that other wallet would have to be built by hand from the
//! phrase.

use std::collections::{HashSet, VecDeque};

use monero_wallet::address::{MoneroAddress, SubaddressIndex};
use monero_wallet::{Scanner, WalletOutput};
use serde_json::{json, Value};

use crate::{ViewKeys, XmrChain, XmrNetwork};

/// Confirmations before an order paid in Monero counts as paid, and before
/// an output may be spent (Monero's own 10-block lock).
pub const CONFIRMATIONS: u64 = 10;
/// How far back the first sync starts.
const FIRST_SYNC_BACK: u64 = 20;
/// Block hashes kept to notice a reorg.
const HASHES_KEPT: usize = 30;

#[derive(Clone)]
struct Owned {
    output: WalletOutput,
    height: u64,
    spent_by: Option<[u8; 32]>,
}

/// Cloned so a sync can run without holding the caller's lock; see [`XmrWallet::merge_sync`].
#[derive(Clone)]
pub struct XmrWallet {
    network: XmrNetwork,
    keys: ViewKeys,
    /// The newest block scanned.
    scanned: Option<u64>,
    /// The chain tip at the last sync.
    tip: Option<u64>,
    hashes: VecDeque<(u64, [u8; 32])>,
    /// Subaddresses `1..next_subaddress` have been handed out; none is reused.
    next_subaddress: u32,
    outputs: Vec<Owned>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    /// Unspent, at least [`CONFIRMATIONS`] deep.
    pub unlocked: u64,
    /// Unspent, not yet deep enough to spend.
    pub pending: u64,
}

/// What arrived at one subaddress.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received {
    pub total: u64,
    /// Of `total`, the part at least [`CONFIRMATIONS`] deep.
    pub confirmed: u64,
    /// Confirmations of the newest output, if any arrived.
    pub confirmations: Option<u64>,
    pub txids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncStep {
    pub tip: u64,
    pub scanned: u64,
    pub found: usize,
    /// Blocks dropped because the chain reorganized.
    pub rolled_back: u64,
}

impl SyncStep {
    pub fn caught_up(&self) -> bool {
        self.scanned >= self.tip
    }
}

impl XmrWallet {
    pub fn new(network: XmrNetwork, keys: ViewKeys) -> Self {
        Self { network, keys, scanned: None, tip: None, hashes: VecDeque::new(), next_subaddress: 1, outputs: Vec::new() }
    }

    pub fn network(&self) -> XmrNetwork {
        self.network
    }

    pub fn keys(&self) -> &ViewKeys {
        &self.keys
    }

    pub fn primary_address(&self) -> MoneroAddress {
        self.keys.primary_address(self.network)
    }

    pub fn scanned(&self) -> Option<u64> {
        self.scanned
    }

    pub fn tip(&self) -> Option<u64> {
        self.tip
    }

    /// A subaddress never handed out before. The caller saves the wallet
    /// before showing it, so a restart cannot hand it out again.
    pub fn issue_subaddress(&mut self) -> (u32, MoneroAddress) {
        let index = self.next_subaddress;
        self.next_subaddress = index.checked_add(1).expect("fewer than 2^32 subaddresses");
        (index, self.keys.subaddress(self.network, index))
    }

    /// Scan up to `max_blocks` new blocks. Call until [`SyncStep::caught_up`].
    pub fn sync_step(&mut self, chain: &dyn XmrChain, max_blocks: u64) -> Result<SyncStep, String> {
        let tip = chain.tip()?;
        let rolled_back = self.check_reorg(chain)?;
        let from = match self.scanned {
            Some(scanned) => scanned + 1,
            None => tip.saturating_sub(FIRST_SYNC_BACK),
        };
        let mut found = 0;
        if from <= tip {
            let to = tip.min(from + max_blocks.max(1) - 1);
            let blocks = chain.blocks(from, to)?;
            if blocks.len() as u64 != to - from + 1 {
                return Err("the Monero node sent the wrong number of blocks".into());
            }
            let mut scanner = Scanner::new(self.keys.view_pair());
            for index in 1..self.next_subaddress {
                scanner.register_subaddress(SubaddressIndex::new(0, index).expect("index is above 0"));
            }
            let mut known: HashSet<[u8; 32]> = self.outputs.iter().map(|owned| owned.output.key().compress().to_bytes()).collect();
            for (number, block) in (from..=to).zip(blocks) {
                let hash = block.block.hash();
                if let Some((_, previous)) = self.hashes.back() {
                    if block.block.header.previous != *previous {
                        return Err("the Monero node sent blocks that do not chain".into());
                    }
                }
                let outputs = scanner.scan(block).map_err(|err| format!("scanning block {number}: {err}"))?;
                // Outputs with an extra timelock are not counted as payment.
                for output in outputs.not_additionally_locked() {
                    // The burning bug: a second output with a key already seen is unspendable.
                    if known.insert(output.key().compress().to_bytes()) {
                        self.outputs.push(Owned { output, height: number, spent_by: None });
                        found += 1;
                    }
                }
                self.hashes.push_back((number, hash));
                while self.hashes.len() > HASHES_KEPT {
                    self.hashes.pop_front();
                }
                self.scanned = Some(number);
            }
        }
        self.tip = Some(tip);
        Ok(SyncStep { tip, scanned: self.scanned.unwrap_or(tip), found, rolled_back })
    }

    /// Drop blocks no longer on the daemon's chain. Returns how many.
    fn check_reorg(&mut self, chain: &dyn XmrChain) -> Result<u64, String> {
        let Some(&(last, hash)) = self.hashes.back() else { return Ok(0) };
        if chain.block_hash(last)? == hash {
            return Ok(0);
        }
        let mut keep = None;
        for &(number, hash) in self.hashes.iter().rev().skip(1) {
            if chain.block_hash(number)? == hash {
                keep = Some(number);
                break;
            }
        }
        let keep = keep.unwrap_or_else(|| self.hashes.front().map_or(0, |(number, _)| number.saturating_sub(1)));
        self.hashes.retain(|(number, _)| *number <= keep);
        self.outputs.retain(|owned| owned.height <= keep);
        self.scanned = Some(keep);
        Ok(last - keep)
    }

    fn confirmations(&self, height: u64) -> u64 {
        self.tip.map_or(0, |tip| (tip + 1).saturating_sub(height))
    }

    pub fn received(&self, subaddress: u32) -> Received {
        let mut received = Received::default();
        let index = SubaddressIndex::new(0, subaddress);
        for owned in self.outputs.iter().filter(|owned| owned.output.subaddress() == index && index.is_some()) {
            let amount = owned.output.commitment().amount;
            let confirmations = self.confirmations(owned.height);
            received.total = received.total.saturating_add(amount);
            if confirmations >= CONFIRMATIONS {
                received.confirmed = received.confirmed.saturating_add(amount);
            }
            received.confirmations = Some(received.confirmations.map_or(confirmations, |seen| seen.min(confirmations)));
            let txid = hex::encode(owned.output.transaction());
            if !received.txids.contains(&txid) {
                received.txids.push(txid);
            }
        }
        received
    }

    pub fn balance(&self) -> Balance {
        let mut balance = Balance::default();
        for owned in self.outputs.iter().filter(|owned| owned.spent_by.is_none()) {
            let amount = owned.output.commitment().amount;
            if self.confirmations(owned.height) >= CONFIRMATIONS {
                balance.unlocked = balance.unlocked.saturating_add(amount);
            } else {
                balance.pending = balance.pending.saturating_add(amount);
            }
        }
        balance
    }

    /// Unspent outputs deep enough to spend, largest first.
    pub fn spendable(&self) -> Vec<WalletOutput> {
        let mut outputs: Vec<&Owned> = self
            .outputs
            .iter()
            .filter(|owned| owned.spent_by.is_none() && self.confirmations(owned.height) >= CONFIRMATIONS)
            .collect();
        outputs.sort_by_key(|owned| std::cmp::Reverse(owned.output.commitment().amount));
        outputs.into_iter().map(|owned| owned.output.clone()).collect()
    }

    /// Take the scan from `synced`, a clone of this wallet that ran
    /// [`XmrWallet::sync_step`]. If a subaddress was handed out meanwhile,
    /// the clone scanned without it, so its scan is dropped and the next sync
    /// covers those blocks again. Returns whether the scan was taken.
    pub fn merge_sync(&mut self, synced: XmrWallet) -> bool {
        if synced.next_subaddress != self.next_subaddress {
            return false;
        }
        let spent: std::collections::HashMap<[u8; 32], [u8; 32]> = self
            .outputs
            .iter()
            .filter_map(|owned| Some((owned.output.key().compress().to_bytes(), owned.spent_by?)))
            .collect();
        self.scanned = synced.scanned;
        self.tip = synced.tip;
        self.hashes = synced.hashes;
        self.outputs = synced.outputs;
        for owned in &mut self.outputs {
            if let Some(txid) = spent.get(&owned.output.key().compress().to_bytes()) {
                owned.spent_by.get_or_insert(*txid);
            }
        }
        true
    }

    /// Record that `inputs` were spent by `txid`, once it is broadcast.
    pub fn mark_spent(&mut self, inputs: &[WalletOutput], txid: [u8; 32]) {
        let keys: HashSet<[u8; 32]> = inputs.iter().map(|output| output.key().compress().to_bytes()).collect();
        for owned in &mut self.outputs {
            if keys.contains(&owned.output.key().compress().to_bytes()) {
                owned.spent_by = Some(txid);
            }
        }
    }

    /// Everything but the keys, which are stored apart in the receive tier.
    pub fn encode(&self) -> Vec<u8> {
        let value = json!({
            "version": 1,
            "network": self.network.as_str(),
            "address": self.primary_address().to_string(),
            "scanned": self.scanned,
            "tip": self.tip,
            "hashes": self.hashes.iter().map(|(number, hash)| json!([number, hex::encode(hash)])).collect::<Vec<_>>(),
            "nextSubaddress": self.next_subaddress,
            "outputs": self.outputs.iter().map(|owned| json!({
                "output": hex::encode(owned.output.serialize()),
                "height": owned.height,
                "spentBy": owned.spent_by.map(hex::encode),
            })).collect::<Vec<_>>(),
        });
        serde_json::to_vec(&value).expect("the wallet is plain json")
    }

    pub fn decode(bytes: &[u8], network: XmrNetwork, keys: ViewKeys) -> Result<Self, String> {
        let bad = || "the Monero wallet file could not be read".to_string();
        let value: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
        let mut wallet = Self::new(network, keys);
        if value["network"].as_str() != Some(network.as_str())
            || value["address"].as_str() != Some(wallet.primary_address().to_string().as_str())
        {
            return Err("the Monero wallet file belongs to another wallet or network".into());
        }
        wallet.scanned = value["scanned"].as_u64();
        wallet.tip = value["tip"].as_u64();
        wallet.next_subaddress = value["nextSubaddress"].as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n >= 1).ok_or_else(bad)?;
        for entry in value["hashes"].as_array().ok_or_else(bad)? {
            let number = entry[0].as_u64().ok_or_else(bad)?;
            wallet.hashes.push_back((number, hash32(&entry[1]).ok_or_else(bad)?));
        }
        for entry in value["outputs"].as_array().ok_or_else(bad)? {
            let bytes = hex::decode(entry["output"].as_str().ok_or_else(bad)?).map_err(|_| bad())?;
            let output = WalletOutput::read(&mut bytes.as_slice()).map_err(|_| bad())?;
            let spent_by = match &entry["spentBy"] {
                Value::Null => None,
                hash => Some(hash32(hash).ok_or_else(bad)?),
            };
            wallet.outputs.push(Owned { output, height: entry["height"].as_u64().ok_or_else(bad)?, spent_by });
        }
        Ok(wallet)
    }
}

fn hash32(value: &Value) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(value.as_str()?, &mut out).ok()?;
    Some(out)
}
