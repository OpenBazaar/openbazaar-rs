//! Scriptless moderated escrow.
//!
//! The funding output is a Taproot key-path spend. Its internal key is the
//! BIP 327 MuSig2 aggregate of the buyer and vendor keys, tweaked with the
//! unspendable BIP 341 taproot tweak (no script tree). The chain sees an
//! ordinary key-path payment for every outcome, including a dispute.
//!
//! # Outcomes
//!
//! * **Happy path.** Buyer and vendor run MuSig2 and key-spend to the vendor.
//!   The moderator is not involved. One of those keys alone cannot produce a
//!   signature that verifies under the aggregate.
//! * **Timeout.** At funding, both parties presign a refund that pays the
//!   buyer and sets `nLockTime` to the agreed height. The moderator is not a
//!   signer. Bitcoin Core's `IsFinalTx` rule rejects the refund until the
//!   chain height is greater than that `nLockTime`.
//! * **Dispute.** Each outcome (pay the buyer, or pay the vendor) is a MuSig2
//!   adaptor signature under the same aggregate key. The adaptor secret is
//!   the sum of two scalars: one derived from the favored party's long-term
//!   key, and one sampled at funding and encrypted to the moderator. The
//!   plaintext moderator half is not retained. [`Escrow::dispute_payload`]
//!   builds the letter (outpoint, claim, settlement terms, encrypted half).
//!   [`moderator_open`] decrypts that half. The favored party then adapts the
//!   signature. Either half alone leaves a signature that does not verify.
//!   The witness is a single 64-byte BIP 340 Schnorr signature.
//!
//! Adaptor signatures come from the `musig2` crate (`musig2::adaptor`), which
//! delegates curve arithmetic to `libsecp256k1`. This crate does not implement
//! a script-leaf spend, `OP_CHECKMULTISIG`, FROST, or BitVM.
//!
//! # Caller obligations
//!
//! * **Key order.** Buyer and vendor compressed keys are sorted lexicographically
//!   before BIP 327 key aggregation. Swapping the party labels does not change
//!   the aggregate. The moderator key is not aggregated.
//! * **Sighash.** Every signature is a BIP 341 key spend of input 0 with
//!   `SIGHASH_DEFAULT` and `Prevouts::All` containing only the funding output.
//!   The witness is 64 bytes. The input sequence is `0xfffffffe`, so absolute
//!   locktime is enforced.
//! * **Locktime.** `timeout_height` is an absolute block height in
//!   `1..500_000_000`. It is the last height at which the refund is not final.
//!   The refund can be mined at height `timeout_height + 1`. Time-based
//!   locktimes are not used. Consensus does not authenticate who broadcasts;
//!   the refund pays the buyer, and both commercial keys are required to
//!   create it.
//! * **Funding.** The caller must create an output whose `scriptPubKey` and
//!   value are [`Escrow::funding_output`], and whose outpoint is the one
//!   passed to [`fund`]. This crate does not broadcast.
//! * **Nonces.** Funding nonces and the moderator adaptor halves are drawn
//!   from the operating-system RNG and discarded before [`fund`] returns.
//!   Do not reuse a MuSig nonce across messages. The happy-path release draws
//!   fresh nonces, distinct from the vendor-wins dispute even though both
//!   pay the vendor the same amount.
//!
//! # State
//!
//! [`Phase::Funded`] is the only non-terminal state. It can move to
//! [`Phase::Completed`], [`Phase::Refunded`], or [`Phase::Disputed`]. Any
//! other transition returns [`Error::InvalidTransition`] and leaves the phase
//! unchanged.

#![forbid(unsafe_code)]

mod crypto;
mod error;
mod payload;
mod session;
mod tx;

pub use error::Error;
pub use payload::Claim;
pub use session::{
    fund, moderator_open, Contribution, Escrow, FundParams, ModeratorShare, ReleaseNonce,
};
pub use tx::is_final_at_height;

#[cfg(test)]
mod tests;

/// Who a dispute ruling pays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Party {
    Buyer = 1,
    Vendor = 2,
}

impl Party {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Party::Buyer),
            2 => Some(Party::Vendor),
            _ => None,
        }
    }
}

/// Explicit escrow state.
///
/// Only `Funded` has outgoing transitions, and each of those is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Funded,
    Completed,
    Refunded,
    Disputed,
}
