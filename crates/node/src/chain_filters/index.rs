//! What the compact filter client has learned, kept on this machine.
//!
//! The index holds the scripts the node follows, the filter commitment of
//! every block it has checked since `start`, the recent header chain, and
//! the transactions from matched blocks that touch a followed script. It
//! does no I/O: the driver feeds it filters, blocks, and header changes, and
//! the chain queries read from it. Nothing in here is ever sent to a peer.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use bdk_wallet::chain::{BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate};
use bdk_wallet::{KeychainKind, Update};
use bitcoin::bip158::{BlockFilter, FilterHash};
use bitcoin::block::Header;
use bitcoin::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::{Block, BlockHash, Network, OutPoint, ScriptBuf, Transaction, TxOut, Txid};

use crate::chain::AddressTx;

/// Headers kept below the scanned tip. A restart re-checks the filters of
/// these blocks, and a reorg deeper than this after a restart is not seen.
pub(crate) const RECENT_HEADERS: u32 = 72;
/// A script added after the scan has passed some blocks is rescanned from
/// this far below the scanned tip. Addresses are handed out shortly before
/// the node starts following them, so this covers the gap.
pub(crate) const NEW_SCRIPT_LOOKBACK: u32 = 144;
/// Own broadcasts that never confirm are forgotten after two weeks.
const MEMPOOL_EXPIRY_SECS: u64 = 14 * 24 * 3600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MatchedBlock {
    pub hash: BlockHash,
    pub time: u32,
    /// Only the transactions that pay or spend a followed script.
    pub txs: Vec<Transaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterCheck {
    /// The block may touch a followed script; fetch it.
    Fetch,
    /// No followed script is in the block, or the block is already stored.
    Skip,
    /// A peer served a different filter for a block already checked. The
    /// first filter's result stands.
    Conflict,
}

#[derive(Clone, Debug)]
pub(crate) struct FilterIndex {
    pub(crate) network: Network,
    /// Scanning began strictly after this block.
    pub(crate) start: BlockId,
    /// Every filter up to here has been checked and its block applied.
    pub(crate) processed: BlockId,
    /// The scan has reached the network's tip at least once.
    pub(crate) caught_up: bool,
    pub(crate) headers: BTreeMap<u32, Header>,
    /// `(block hash, filter hash)` of each block checked since `start`.
    pub(crate) commitments: BTreeMap<u32, (BlockHash, FilterHash)>,
    /// Followed scripts and the generation that added each one.
    pub(crate) scripts: BTreeMap<ScriptBuf, u64>,
    pub(crate) blocks: BTreeMap<u32, MatchedBlock>,
    /// Transactions this node broadcast that no matched block holds yet.
    pub(crate) mempool: BTreeMap<Txid, (Transaction, u64)>,
    generation: u64,
    /// Every script with a generation at or below this has been scanned to `processed`.
    covered: u64,
    /// Rescan after this height once the client can take a rescan.
    rescan_from: Option<u32>,
    /// A rescan sent to the client: its generation, start, and whether its first filter arrived.
    in_flight: Option<(u64, u32, bool)>,
    /// Filters checked out of order, waiting for the ones below them.
    ahead: BTreeMap<u32, BlockHash>,
}

impl FilterIndex {
    pub(crate) fn new(network: Network, start: BlockId) -> Self {
        Self {
            network,
            start,
            processed: start,
            caught_up: false,
            headers: BTreeMap::new(),
            commitments: BTreeMap::new(),
            scripts: BTreeMap::new(),
            blocks: BTreeMap::new(),
            mempool: BTreeMap::new(),
            generation: 0,
            covered: 0,
            rescan_from: None,
            in_flight: None,
            ahead: BTreeMap::new(),
        }
    }

    /// Follow these scripts. Returns `true` when any is new and a rescan was queued.
    pub(crate) fn watch(&mut self, scripts: impl IntoIterator<Item = ScriptBuf>) -> bool {
        let mut added = false;
        for script in scripts {
            if self.scripts.contains_key(&script) {
                continue;
            }
            if !added {
                self.generation += 1;
                added = true;
            }
            self.scripts.insert(script, self.generation);
        }
        if !added {
            return false;
        }
        if self.commitments.is_empty() {
            // Nothing checked yet: the first pass covers the new scripts.
            self.covered = self.generation;
            return false;
        }
        let from = self.processed.height.saturating_sub(NEW_SCRIPT_LOOKBACK).max(self.start.height);
        self.rescan_from = Some(self.rescan_from.map_or(from, |queued| queued.min(from)));
        true
    }

    /// The queued rescan, if the client can take one now. The caller sends
    /// it as `rescan_from(height)`; filters after `height` come again.
    pub(crate) fn take_rescan(&mut self) -> Option<u32> {
        if self.in_flight.is_some() || !self.caught_up {
            return None;
        }
        let from = self.rescan_from.take()?;
        self.in_flight = Some((self.generation, from, false));
        Some(from)
    }

    pub(crate) fn is_ready(&self, script: &ScriptBuf) -> bool {
        self.caught_up && self.scripts.get(script).is_some_and(|generation| *generation <= self.covered)
    }

    /// A header joined the chain of most work.
    pub(crate) fn connect(&mut self, height: u32, header: Header) {
        if height <= self.start.height {
            return;
        }
        self.headers.insert(height, header);
        let keep_from = self.processed.height.saturating_sub(RECENT_HEADERS);
        self.headers = self.headers.split_off(&keep_from);
    }

    /// Blocks at and above the lowest `removed` height left the chain.
    /// Their transactions go back to unconfirmed until a new block holds them.
    pub(crate) fn reorganize(&mut self, accepted: &[(u32, Header)], removed: &[(u32, Header)], now: u64) {
        let Some(fork) = removed.iter().map(|(height, _)| *height).min() else {
            return;
        };
        for (_, block) in self.blocks.split_off(&fork) {
            for tx in block.txs {
                self.mempool.entry(tx.compute_txid()).or_insert((tx, now));
            }
        }
        self.commitments.split_off(&fork);
        self.headers.split_off(&fork);
        self.ahead.split_off(&fork);
        if self.processed.height >= fork {
            let below = fork - 1;
            let hash = accepted
                .iter()
                .find(|(height, _)| *height == fork)
                .map(|(_, header)| header.prev_blockhash)
                .or_else(|| self.headers.get(&below).map(Header::block_hash))
                .unwrap_or(self.start.hash);
            self.processed = BlockId { height: below.max(self.start.height), hash };
        }
        for (height, header) in accepted {
            self.connect(*height, *header);
        }
    }

    /// Check one block's filter against every followed script and record its commitment.
    pub(crate) fn check_filter(&mut self, height: u32, hash: BlockHash, content: &[u8]) -> FilterCheck {
        if let Some((_, from, seen)) = &mut self.in_flight {
            if height == *from + 1 {
                *seen = true;
            }
        }
        let filter_hash = FilterHash::hash(content);
        match self.commitments.get(&height) {
            Some((known, earlier)) if *known == hash && *earlier != filter_hash => return FilterCheck::Conflict,
            _ => {
                self.commitments.insert(height, (hash, filter_hash));
            }
        }
        if self.blocks.get(&height).is_some_and(|block| block.hash == hash) {
            return FilterCheck::Skip;
        }
        let filter = BlockFilter::new(content);
        let query = self.scripts.keys().map(|script| script.as_bytes());
        match filter.match_any(&hash, query) {
            Ok(true) => FilterCheck::Fetch,
            Ok(false) => FilterCheck::Skip,
            // An undecodable filter cannot rule the block out.
            Err(_) => FilterCheck::Fetch,
        }
    }

    /// Keep the transactions of a matched block that touch a followed script.
    pub(crate) fn apply_block(&mut self, height: u32, block: &Block) {
        let hash = block.block_hash();
        let mut ours: HashSet<OutPoint> = self.owned_outpoints();
        let mut txs = Vec::new();
        for tx in &block.txdata {
            let txid = tx.compute_txid();
            let pays = tx.output.iter().any(|out| self.scripts.contains_key(&out.script_pubkey));
            let spends = tx.input.iter().any(|input| ours.contains(&input.previous_output));
            if pays || spends {
                for (vout, out) in tx.output.iter().enumerate() {
                    if self.scripts.contains_key(&out.script_pubkey) {
                        ours.insert(OutPoint { txid, vout: vout as u32 });
                    }
                }
                self.mempool.remove(&txid);
                txs.push(tx.clone());
            }
        }
        if txs.is_empty() {
            return;
        }
        self.blocks.insert(height, MatchedBlock { hash, time: block.header.time, txs });
    }

    /// Mark a filter done. `processed` moves only over an unbroken run of done filters.
    pub(crate) fn filter_done(&mut self, height: u32, hash: BlockHash) {
        if height <= self.processed.height {
            return;
        }
        self.ahead.insert(height, hash);
        while let Some(hash) = self.ahead.remove(&(self.processed.height + 1)) {
            self.processed = BlockId { height: self.processed.height + 1, hash };
        }
        let keep_from = self.processed.height.saturating_sub(RECENT_HEADERS);
        self.headers = self.headers.split_off(&keep_from);
    }

    /// The client checked every filter up to its tip.
    pub(crate) fn synced(&mut self) {
        self.caught_up = true;
        match self.in_flight {
            Some((generation, _, true)) => {
                self.covered = generation;
                self.in_flight = None;
            }
            Some(_) => {}
            None if self.rescan_from.is_none() => self.covered = self.generation,
            None => {}
        }
    }

    pub(crate) fn tip(&self) -> Result<u32, String> {
        if !self.caught_up {
            return Err(format!(
                "compact block filters are syncing (checked through block {})",
                self.processed.height
            ));
        }
        Ok(self.processed.height)
    }

    pub(crate) fn add_mempool(&mut self, tx: Transaction, now: u64) {
        self.mempool.insert(tx.compute_txid(), (tx, now));
    }

    pub(crate) fn expire_mempool(&mut self, now: u64) {
        self.mempool.retain(|_, (_, seen)| now.saturating_sub(*seen) < MEMPOOL_EXPIRY_SECS);
    }

    pub(crate) fn address_txs(&self, script: &ScriptBuf) -> Vec<AddressTx> {
        self.all_txs()
            .filter_map(|(tx, height)| {
                let outputs: Vec<(u32, u64)> = tx
                    .output
                    .iter()
                    .enumerate()
                    .filter(|(_, out)| out.script_pubkey == *script)
                    .map(|(vout, out)| (vout as u32, out.value.to_sat()))
                    .collect();
                (!outputs.is_empty()).then(|| AddressTx { txid: tx.compute_txid(), outputs, height })
            })
            .collect()
    }

    /// Outputs of transactions this index holds. Any other outpoint is unknown here.
    pub(crate) fn output(&self, outpoint: &OutPoint) -> Option<(TxOut, Option<u32>)> {
        self.all_txs()
            .find(|(tx, _)| tx.compute_txid() == outpoint.txid)
            .and_then(|(tx, height)| tx.output.get(outpoint.vout as usize).cloned().map(|out| (out, height)))
    }

    /// A BDK update for a wallet whose scripts are `spks` (with keychain indexes when known).
    pub(crate) fn wallet_update(&self, spks: &[(Option<(KeychainKind, u32)>, ScriptBuf)]) -> Update {
        let wanted: HashSet<&ScriptBuf> = spks.iter().map(|(_, script)| script).collect();
        let mut owned: HashSet<OutPoint> = HashSet::new();
        let mut tx_update = TxUpdate::default();
        let mut anchor_blocks = BTreeSet::new();
        let mut last_active: BTreeMap<KeychainKind, u32> = BTreeMap::new();
        for (tx, height) in self.all_txs() {
            let txid = tx.compute_txid();
            let mut relevant = tx.input.iter().any(|input| owned.contains(&input.previous_output));
            for (vout, out) in tx.output.iter().enumerate() {
                if !wanted.contains(&out.script_pubkey) {
                    continue;
                }
                relevant = true;
                owned.insert(OutPoint { txid, vout: vout as u32 });
                if let Some(Some((keychain, index))) = spks.iter().find(|(_, s)| *s == out.script_pubkey).map(|(k, _)| *k) {
                    let entry = last_active.entry(keychain).or_insert(index);
                    *entry = (*entry).max(index);
                }
            }
            if !relevant {
                continue;
            }
            match height.and_then(|height| self.blocks.get(&height).map(|block| (height, block))) {
                Some((height, block)) => {
                    let block_id = BlockId { height, hash: block.hash };
                    anchor_blocks.insert(block_id);
                    tx_update.anchors.insert((
                        ConfirmationBlockTime { block_id, confirmation_time: block.time as u64 },
                        txid,
                    ));
                }
                None => {
                    let seen = self.mempool.get(&txid).map_or(0, |(_, seen)| *seen);
                    tx_update.seen_ats.insert((txid, seen));
                }
            }
            tx_update.txs.push(Arc::new(tx.clone()));
        }
        Update {
            tx_update,
            last_active_indices: last_active,
            chain: Some(self.checkpoint(anchor_blocks)),
        }
    }

    /// Genesis, the blocks that anchor transactions, and the recent headers up to `processed`.
    fn checkpoint(&self, mut blocks: BTreeSet<BlockId>) -> CheckPoint {
        blocks.insert(BlockId { height: 0, hash: genesis_block(self.network).block_hash() });
        if self.start.height > 0 {
            blocks.insert(self.start);
        }
        for (height, header) in self.headers.range(..=self.processed.height) {
            blocks.insert(BlockId { height: *height, hash: header.block_hash() });
        }
        blocks.insert(self.processed);
        let mut by_height: BTreeMap<u32, BlockHash> = BTreeMap::new();
        for block in blocks {
            by_height.insert(block.height, block.hash);
        }
        CheckPoint::from_block_ids(by_height.into_iter().map(|(height, hash)| BlockId { height, hash }))
            .expect("heights are ascending and start at genesis")
    }

    /// Stored transactions with their block heights, then own unconfirmed broadcasts.
    fn all_txs(&self) -> impl Iterator<Item = (&Transaction, Option<u32>)> {
        self.blocks
            .iter()
            .flat_map(|(height, block)| block.txs.iter().map(move |tx| (tx, Some(*height))))
            .chain(self.mempool.values().map(|(tx, _)| (tx, None)))
    }

    fn owned_outpoints(&self) -> HashSet<OutPoint> {
        let mut owned = HashSet::new();
        for (tx, _) in self.all_txs() {
            let txid = tx.compute_txid();
            for (vout, out) in tx.output.iter().enumerate() {
                if self.scripts.contains_key(&out.script_pubkey) {
                    owned.insert(OutPoint { txid, vout: vout as u32 });
                }
            }
        }
        owned
    }

    /// Scripts whose rescan has not finished, and the height it starts after.
    pub(crate) fn pending(&self) -> (Vec<ScriptBuf>, Option<u32>) {
        let from = [self.rescan_from, self.in_flight.map(|(_, from, _)| from)].into_iter().flatten().min();
        let scripts = self
            .scripts
            .iter()
            .filter(|(_, generation)| **generation > self.covered)
            .map(|(script, _)| script.clone())
            .collect();
        (scripts, from)
    }

    /// Re-queue a rescan that was unfinished when the state was saved.
    pub(crate) fn restore_pending(&mut self, scripts: Vec<ScriptBuf>, from: Option<u32>) {
        let Some(from) = from.filter(|_| !scripts.is_empty()) else {
            return;
        };
        self.generation = 1;
        for script in scripts {
            self.scripts.insert(script, 1);
        }
        self.rescan_from = Some(from);
    }

    /// Recent headers up to `processed`, oldest first, for the client's chain state.
    pub(crate) fn snapshot(&self) -> Vec<(u32, Header)> {
        self.headers
            .range(..=self.processed.height)
            .map(|(height, header)| (*height, *header))
            .collect()
    }
}
