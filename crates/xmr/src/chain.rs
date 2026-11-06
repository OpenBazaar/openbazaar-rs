//! What the wallet needs from a Monero daemon.

use monero_wallet::interface::{FeeRate, ScannableBlock};
use monero_wallet::transaction::Transaction;
use monero_wallet::{OutputWithDecoys, WalletOutput};

use crate::BackendStatus;

/// A monerod the user chose, or a test double. Every call is blocking and
/// may go over Tor, so the node calls it off the HTTP thread except for the
/// fee estimate in a quote.
pub trait XmrChain: Send + Sync {
    fn status(&self) -> BackendStatus;
    /// The number of the newest block.
    fn tip(&self) -> Result<u64, String>;
    fn block_hash(&self, number: u64) -> Result<[u8; 32], String>;
    /// Blocks `from..=to`, each with its transactions, for scanning.
    fn blocks(&self, from: u64, to: u64) -> Result<Vec<ScannableBlock>, String>;
    fn fee_rate(&self) -> Result<FeeRate, String>;
    /// A ring of [`crate::RING_LEN`] members for each output, as of block `height`.
    fn decoys(&self, outputs: Vec<WalletOutput>, height: u64) -> Result<Vec<OutputWithDecoys>, String>;
    fn publish(&self, tx: &Transaction) -> Result<(), String>;
}
