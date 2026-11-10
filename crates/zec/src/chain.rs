//! What the wallet needs from a chain source.

use crate::proto::CompactBlock;

/// A Zaino or lightwalletd the user chose, or a test double. Every call is
/// blocking and may go over Tor, so the node calls it off the HTTP thread.
///
/// Hashes are the block hash bytes as compact blocks carry them. A source
/// only ever hands over whole blocks; it is never asked about an address or
/// a transaction.
pub trait ZecChain: Send + Sync {
    /// The height of the newest block.
    fn tip(&self) -> Result<u64, String>;
    fn block_hash(&self, height: u64) -> Result<[u8; 32], String>;
    /// Compact blocks `from..=to`, oldest first.
    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String>;
}
