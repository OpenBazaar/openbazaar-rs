//! Incoming payments still in the daemon's mempool.
//!
//! Shown to the user and nothing else: they never count toward a balance,
//! spendable outputs, or an order being paid, and they are never saved. A
//! transaction is fetched once and remembered by hash while it stays in the
//! pool, so an unchanged pool costs one small request per round (over Tor).

use std::collections::{HashMap, HashSet};

use monero_wallet::block::{Block, BlockHeader};
use monero_wallet::interface::ScannableBlock;
use monero_wallet::transaction::{Input, Pruned, Timelock, Transaction, TransactionPrefix};

use crate::{XmrChain, XmrWallet};

/// One output of a pool transaction that pays this wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub txid: [u8; 32],
    pub amount: u64,
    /// 0 for the primary address.
    pub subaddress: u32,
}

#[derive(Clone, Debug, Default)]
pub struct PoolWatch {
    /// Every pool transaction checked so far, with what paid this wallet (often nothing).
    checked: HashMap<[u8; 32], Vec<Incoming>>,
    /// Subaddresses the checks covered. A newer subaddress needs the pool checked again.
    subaddresses: u32,
}

impl PoolWatch {
    /// Ask the daemon for its pool and check the transactions not seen before.
    /// Transactions that left the pool are forgotten.
    pub fn refresh(&mut self, wallet: &XmrWallet, chain: &dyn XmrChain) -> Result<(), String> {
        let hashes = chain.pool_hashes()?;
        if self.subaddresses != wallet.next_subaddress() {
            self.checked.clear();
            self.subaddresses = wallet.next_subaddress();
        }
        let current: HashSet<[u8; 32]> = hashes.iter().copied().collect();
        self.checked.retain(|hash, _| current.contains(hash));
        let new: Vec<[u8; 32]> = hashes.into_iter().filter(|hash| !self.checked.contains_key(hash)).collect();
        if new.is_empty() {
            return Ok(());
        }
        for (hash, tx) in chain.pool_transactions(&new)? {
            if !current.contains(&hash) {
                return Err("the Monero node sent a pool transaction that was not asked for".into());
            }
            let found = scan(wallet, hash, tx)?;
            self.checked.insert(hash, found);
        }
        Ok(())
    }

    /// What pays the wallet from the pool, leaving out transactions already
    /// mined into the wallet and its own spends (change).
    pub fn incoming(&self, wallet: &XmrWallet) -> Vec<Incoming> {
        let mut out: Vec<Incoming> = self
            .checked
            .values()
            .flatten()
            .filter(|incoming| !wallet.holds_transaction(&incoming.txid) && !wallet.spent_in(&incoming.txid))
            .cloned()
            .collect();
        out.sort_by_key(|incoming| (incoming.txid, incoming.subaddress));
        out
    }

    pub fn total(&self, wallet: &XmrWallet) -> u64 {
        self.incoming(wallet).iter().fold(0, |sum, incoming| sum.saturating_add(incoming.amount))
    }

    /// Pool transactions remembered, matching or not.
    pub fn checked(&self) -> usize {
        self.checked.len()
    }
}

/// Scan one transaction by wrapping it in a block of its own. The output
/// indexes that come out are made up, which is fine: these outputs are only shown.
fn scan(wallet: &XmrWallet, hash: [u8; 32], tx: Transaction<Pruned>) -> Result<Vec<Incoming>, String> {
    let miner = Transaction::V2 {
        prefix: TransactionPrefix { additional_timelock: Timelock::None, inputs: vec![Input::Gen(0)], outputs: Vec::new(), extra: Vec::new() },
        proofs: None,
    };
    let header = BlockHeader { hardfork_version: 16, hardfork_signal: 16, timestamp: 0, previous: [0; 32], nonce: 0 };
    let block = Block::new(header, miner, vec![hash]).ok_or("a one-transaction block could not be built")?;
    let scannable = ScannableBlock { block, transactions: vec![tx], output_index_for_first_ringct_output: Some(0) };
    let outputs = wallet.scanner().scan(scannable).map_err(|err| format!("scanning pool transaction: {err}"))?;
    Ok(outputs
        .not_additionally_locked()
        .into_iter()
        .map(|output| Incoming {
            txid: hash,
            amount: output.commitment().amount,
            subaddress: output.subaddress().map_or(0, |index| index.address()),
        })
        .collect())
}
