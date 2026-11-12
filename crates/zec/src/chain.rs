//! What the wallet needs from a chain source.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::proto::CompactBlock;
use crate::BackendStatus;

/// A Zaino or lightwalletd the user chose, or a test double. Every call is
/// blocking and may go over Tor, so the node calls it off the HTTP thread.
///
/// Hashes are the block hash bytes as compact blocks carry them. A source
/// only ever hands over whole blocks; it is never asked about an address or
/// a transaction.
pub trait ZecChain: Send + Sync {
    /// What `GET /network` shows about this source.
    fn status(&self) -> BackendStatus;
    /// The height of the newest block.
    fn tip(&self) -> Result<u64, String>;
    fn block_hash(&self, height: u64) -> Result<[u8; 32], String>;
    /// Compact blocks `from..=to`, oldest first.
    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String>;
}

/// One tip and each block hash, reused for every wallet in a sync round.
pub struct SharedTip<'a> {
    inner: &'a dyn ZecChain,
    tip: Mutex<Option<Result<u64, String>>>,
    hashes: Mutex<HashMap<u64, Result<[u8; 32], String>>>,
}

impl<'a> SharedTip<'a> {
    pub fn new(inner: &'a dyn ZecChain) -> Self {
        Self {
            inner,
            tip: Mutex::new(None),
            hashes: Mutex::new(HashMap::new()),
        }
    }
}

impl ZecChain for SharedTip<'_> {
    fn status(&self) -> BackendStatus {
        self.inner.status()
    }

    fn tip(&self) -> Result<u64, String> {
        let mut slot = self.tip.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = slot.as_ref() {
            return cached.clone();
        }
        let got = self.inner.tip();
        *slot = Some(got.clone());
        got
    }

    fn block_hash(&self, height: u64) -> Result<[u8; 32], String> {
        let mut slot = self.hashes.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = slot.get(&height) {
            return cached.clone();
        }
        let got = self.inner.block_hash(height);
        slot.insert(height, got.clone());
        got
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String> {
        self.inner.blocks(from, to)
    }
}
