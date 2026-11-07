//! Review M6: limits on dispute letters before any expensive check.
//!
//! Anyone can write a letter that passes the cheap checks: the terms and
//! the claimant's signature only need keys the writer made up. What costs
//! the moderator is the chain lookup (on compact block filters, a rescan
//! for a new script) and the share proofs (512 bit proofs and a link
//! proof, about 1,500 scalar multiplications). [`DisputeLimits::admit`] is
//! the gate [`openbazaar_escrow::Moderator::review_gated`] runs right before
//! those:
//!
//! * a global token bucket of [`BURST`] letters, refilled one every
//!   [`REFILL`];
//! * at most [`PER_ESCROW`] distinct letters per escrow (terms digest and
//!   outpoint) in any [`PER_ESCROW_WINDOW`].
//!
//! A letter already admitted is admitted again for free, so a letter that
//! waits in the inbox for the chain view is not charged twice. Letters whose
//! proofs already verified skip the gate entirely (the escrow crate's
//! letter cache).
//!
//! Batch verification is not used. The proofs carry challenges rather than
//! commitments, so their equations cannot be summed under random weights,
//! and the secp256k1 bindings expose no multi-scalar multiplication. A
//! commitment-form encoding would let the 513 proofs of one share be checked
//! as one multi-exponentiation; that is a letter format change, listed in the
//! review packet.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use openbazaar_escrow::{Error, RulingKey};

pub(crate) const BURST: u32 = 20;
pub(crate) const REFILL: Duration = Duration::from_secs(6);
pub(crate) const PER_ESCROW: usize = 4;
pub(crate) const PER_ESCROW_WINDOW: Duration = Duration::from_secs(24 * 3600);
const ADMITTED_MAX: usize = 1024;

struct State {
    tokens: u32,
    refilled: u64,
    per_escrow: HashMap<RulingKey, Vec<u64>>,
    admitted: VecDeque<[u8; 32]>,
}

pub(crate) struct DisputeLimits {
    burst: u32,
    state: Mutex<State>,
}

impl Default for DisputeLimits {
    fn default() -> Self {
        Self::new(BURST)
    }
}

impl DisputeLimits {
    pub(crate) fn new(burst: u32) -> Self {
        Self {
            burst,
            state: Mutex::new(State {
                tokens: burst,
                refilled: 0,
                per_escrow: HashMap::new(),
                admitted: VecDeque::new(),
            }),
        }
    }

    /// Admit the letter with digest `letter` for escrow `key` at `now`
    /// (unix seconds), or refuse it with [`Error::RateLimited`].
    pub(crate) fn admit(&self, letter: [u8; 32], key: &RulingKey, now: u64) -> Result<(), Error> {
        let mut guard = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let state = &mut *guard;
        if state.admitted.contains(&letter) {
            return Ok(());
        }
        let refill = REFILL.as_secs().max(1);
        if state.refilled == 0 {
            state.refilled = now;
        }
        let earned = now.saturating_sub(state.refilled) / refill;
        if earned > 0 {
            state.tokens = state.tokens.saturating_add(earned as u32).min(self.burst);
            state.refilled += earned * refill;
        }
        let window = PER_ESCROW_WINDOW.as_secs();
        let recent = state.per_escrow.entry(*key).or_default();
        recent.retain(|at| now.saturating_sub(*at) < window);
        if recent.len() >= PER_ESCROW {
            return Err(Error::RateLimited);
        }
        if state.tokens == 0 {
            return Err(Error::RateLimited);
        }
        state.tokens -= 1;
        state.per_escrow.entry(*key).or_default().push(now);
        if state.per_escrow.len() > ADMITTED_MAX {
            state.per_escrow.retain(|_, times| times.iter().any(|at| now.saturating_sub(*at) < window));
        }
        state.admitted.push_back(letter);
        if state.admitted.len() > ADMITTED_MAX {
            state.admitted.pop_front();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::OutPoint;

    fn key(n: u8) -> RulingKey {
        RulingKey {
            terms_digest: [n; 32],
            outpoint: OutPoint::null(),
        }
    }

    #[test]
    fn the_bucket_refills_and_an_admitted_letter_passes_again() {
        let limits = DisputeLimits::new(3);
        let now = 1_000;
        for n in 0..3u8 {
            limits.admit([n; 32], &key(n), now).unwrap();
        }
        assert_eq!(limits.admit([9; 32], &key(9), now), Err(Error::RateLimited));
        limits.admit([0; 32], &key(0), now).unwrap();
        assert_eq!(limits.admit([9; 32], &key(9), now + REFILL.as_secs() - 1), Err(Error::RateLimited));
        limits.admit([9; 32], &key(9), now + REFILL.as_secs()).unwrap();
    }

    #[test]
    fn one_escrow_gets_a_few_letters_a_day() {
        let limits = DisputeLimits::new(100);
        let now = 5_000;
        for n in 0..PER_ESCROW as u8 {
            limits.admit([n; 32], &key(1), now).unwrap();
        }
        assert_eq!(limits.admit([50; 32], &key(1), now), Err(Error::RateLimited));
        limits.admit([51; 32], &key(2), now).unwrap();
        limits.admit([50; 32], &key(1), now + PER_ESCROW_WINDOW.as_secs()).unwrap();
    }
}
