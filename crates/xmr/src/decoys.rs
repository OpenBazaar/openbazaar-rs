//! A decoy source with no network: a fixed output distribution, the real
//! outputs it was told about, and random points for every other index.
//!
//! Used to size a transaction for its fee before the real decoys are
//! fetched (the weight depends on the ring size, not on which outputs are in
//! the ring), and by the mock chain.

use std::collections::HashMap;
use std::future::Future;
use std::ops::{Bound, RangeBounds};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use curve25519_dalek::constants::ED25519_BASEPOINT_TABLE;
use monero_wallet::ed25519::{Point, Scalar};
use monero_wallet::interface::{
    EvaluateUnlocked, InterfaceError, ProvidesBlockchainMeta, ProvidesUnvalidatedDecoys, TransactionsError,
};
use monero_wallet::{OutputWithDecoys, WalletOutput};
use rand_core::OsRng;

use crate::RING_LEN;

pub(crate) struct DecoySource {
    /// Cumulative RingCT outputs at the end of each block.
    pub distribution: Vec<u64>,
    pub known: HashMap<u64, [Point; 2]>,
}

/// Blocks in the distribution made up for a fee estimate.
const ESTIMATE_BLOCKS: u64 = 2000;

impl DecoySource {
    /// A distribution that places every one of `outputs` well below the tip.
    pub(crate) fn for_estimate(outputs: &[WalletOutput]) -> Self {
        let highest = outputs.iter().map(WalletOutput::index_on_blockchain).max().unwrap_or(0);
        let per_block = (highest + 1).div_ceil(ESTIMATE_BLOCKS - 20) + 1;
        Self {
            distribution: (1..=ESTIMATE_BLOCKS).map(|block| block * per_block).collect(),
            known: outputs.iter().map(|output| (output.index_on_blockchain(), ring_member(output))).collect(),
        }
    }

    pub(crate) fn tip(&self) -> usize {
        self.distribution.len().saturating_sub(1)
    }

    pub(crate) fn select(&self, outputs: Vec<WalletOutput>, height: usize) -> Result<Vec<OutputWithDecoys>, String> {
        let mut out = Vec::with_capacity(outputs.len());
        for output in outputs {
            let picked = ready(OutputWithDecoys::new(&mut OsRng, self, RING_LEN, height, output));
            out.push(picked.map_err(|err| err.to_string())?);
        }
        Ok(out)
    }
}

pub(crate) fn ring_member(output: &WalletOutput) -> [Point; 2] {
    [output.key(), output.commitment().commit()]
}

pub(crate) fn random_point() -> Point {
    Point::from(&Scalar::random(&mut OsRng).into() * ED25519_BASEPOINT_TABLE)
}

impl ProvidesBlockchainMeta for DecoySource {
    fn latest_block_number(&self) -> impl Send + Future<Output = Result<usize, InterfaceError>> {
        let tip = self.tip();
        async move { Ok(tip) }
    }
}

impl ProvidesUnvalidatedDecoys for DecoySource {
    fn ringct_output_distribution(
        &self,
        range: impl Send + RangeBounds<usize>,
    ) -> impl Send + Future<Output = Result<Vec<u64>, InterfaceError>> {
        let start = match range.start_bound() {
            Bound::Included(n) => *n,
            Bound::Excluded(n) => n + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(n) => n + 1,
            Bound::Excluded(n) => *n,
            Bound::Unbounded => self.distribution.len(),
        }
        .min(self.distribution.len());
        let slice = self.distribution.get(start..end).map(<[u64]>::to_vec).unwrap_or_default();
        async move { Ok(slice) }
    }

    fn unlocked_ringct_outputs(
        &self,
        indexes: &[u64],
        _evaluate_unlocked: EvaluateUnlocked,
    ) -> impl Send + Future<Output = Result<Vec<Option<[Point; 2]>>, TransactionsError>> {
        let outputs = indexes
            .iter()
            .map(|index| Some(self.known.get(index).copied().unwrap_or_else(|| [random_point(), random_point()])))
            .collect();
        async move { Ok(outputs) }
    }
}

/// Run a future that never waits (every source here answers at once).
pub(crate) fn ready<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}
