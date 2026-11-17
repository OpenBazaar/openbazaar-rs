//! What the wallet needs from a chain source.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::proto::CompactBlock;
use crate::BackendStatus;

/// The Orchard and Ironwood note-commitment frontiers after a block, as
/// `GetTreeState` returns them. The bytes are a commitment-tree encoding.
/// They are not a list of notes, so asking for them does not reveal which
/// outputs are ours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitmentTrees {
    pub orchard: Vec<u8>,
    pub ironwood: Vec<u8>,
}

/// A Zaino or lightwalletd the user chose, or a test double. Every call is
/// blocking and may go over Tor, so the node calls it off the HTTP thread.
///
/// Hashes are the block hash bytes as compact blocks carry them. A source
/// hands over whole blocks and the commitment-tree frontier at a height.
/// It is never asked about an address, and never for a transaction by id.
/// Broadcasting a transaction we built is the one exception.
pub trait ZecChain: Send + Sync {
    /// What `GET /network` shows about this source.
    fn status(&self) -> BackendStatus;
    /// The height of the newest block.
    fn tip(&self) -> Result<u64, String>;
    fn block_hash(&self, height: u64) -> Result<[u8; 32], String>;
    /// Compact blocks `from..=to`, oldest first.
    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String>;
    /// The note-commitment frontiers after `height`.
    fn tree_state(&self, height: u64) -> Result<CommitmentTrees, String>;
    /// Broadcast a raw transaction. Returns its id, in the byte order the
    /// compact block carries (not the reversed hex wallets display).
    fn submit(&self, raw: &[u8]) -> Result<[u8; 32], String>;
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

    fn tree_state(&self, height: u64) -> Result<CommitmentTrees, String> {
        self.inner.tree_state(height)
    }

    fn submit(&self, raw: &[u8]) -> Result<[u8; 32], String> {
        self.inner.submit(raw)
    }
}
