//! Scriptless two-party escrow with a bounded moderator.
//!
//! The funding output is a Taproot key-path spend. Its internal key is the
//! BIP 327 MuSig2 aggregate of the buyer's and vendor's per-order keys, with
//! the unspendable BIP 341 tweak (no script tree). The moderator key is never
//! aggregated. Every outcome is an ordinary 64-byte key-path signature.
//!
//! # Who can do what
//!
//! Every spend needs both commercial keys. All of them are signed before the
//! buyer funds, except the happy-path release:
//!
//! | spend          | `nLockTime`           | pays                        | moderator |
//! |----------------|-----------------------|-----------------------------|-----------|
//! | release        | 0                     | vendor                      | no        |
//! | vendor claim   | `vendor_claim_height` | vendor                      | no        |
//! | buyer refund   | `refund_height`       | buyer                       | no        |
//! | buyer wins     | 0                     | buyer + `moderator_fee`     | picks it  |
//! | vendor wins    | 0                     | vendor + `moderator_fee`    | picks it  |
//!
//! The two dispute outcomes are MuSig2 adaptor signatures. Branch `X` (favoring
//! party `X`) has adaptor point `T_X + M_X`, where:
//!
//! * `T_X` is derived from `X`'s per-order key and proved with a Schnorr proof
//!   of knowledge, so `X` cannot pick it to cancel `M_X`;
//! * `M_X` is sampled by the party that *loses* branch `X` and verifiably
//!   encrypted to the moderator key (see [`VerifiableShare`]). `X` checks the
//!   proof before it signs anything that leads to funding.
//!
//! So the moderator can only reveal one of the two halves, and only for
//! payouts both parties signed. It cannot redirect funds, change amounts, or
//! sign alone. The favored party alone cannot finish its branch, and the
//! losing party cannot finish a branch that pays it.
//!
//! # Protocol
//!
//! ```text
//! round 0  buyer & vendor  EscrowTerms::sign, SignedTerms::new
//! round 1  buyer  -> vendor  BuyerOpen         (outpoint, nonces, T_b + PoK, enc(M_vendor_wins) + proof)
//! round 2  vendor -> buyer   VendorReply       (nonces, T_v + PoK, enc(M_buyer_wins) + proof,
//!                                               partials: refund, buyer-wins)
//! round 3  buyer  -> vendor  BuyerSignatures   (partials: vendor claim, vendor-wins)
//! release  vendor -> buyer   ReleaseNonce::public
//!          buyer  -> vendor  ReleaseSignature
//! dispute  claimant -> moderator   dispute letter (self-verifying)
//!          moderator -> claimant   ModeratorRuling
//! ```
//!
//! [`BuyerSession`], [`BuyerAwaitingReply`], [`VendorSession`], and
//! [`VendorAwaitingSignatures`] each hold one party's secret only. The buyer's
//! funding PSBT is moved in at round 1 and is only returned by
//! [`BuyerEscrow::funding_psbt`], after the refund has verified against the
//! aggregate key and the exact funding outpoint and value.
//!
//! # Timeline
//!
//! Let `D = dispute_deadline`, `V = vendor_claim_height`, `R = refund_height`,
//! with `D < V < R` (checked against [`Policy`] gaps, 144 blocks each by
//! default). A transaction with `nLockTime = h` can first be mined at `h + 1`.
//!
//! ```text
//! funding ──── ship, deliver ──── D ── ruling window ── V ── claim window ── R ──▶
//!   │                              │                     │                    │
//!   │ release any time             │ last height a       │ vendor claim       │ buyer refund
//!   │ dispute letters accepted ────┘ letter is accepted  │ minable at V + 1   │ minable at R + 1
//!   │ dispute outcomes valid at any height once ruled    │                    │
//! ```
//!
//! * Before `D`, a buyer who did not get the goods opens a dispute. The
//!   moderator must rule, and the winner must confirm, before `V + 1`.
//! * A buyer who got the goods and stalls loses at `V + 1`: the vendor claim
//!   becomes final before the refund.
//! * The refund at `R` only matters if the vendor vanished and never claimed.
//!
//! The default is the vendor from order acceptance onward. A second presigned
//! set after a shipping attestation would not help: a buyer who wants to
//! cheat would simply not sign it, and without a third-party oracle a
//! shipping attestation is not a cryptographic event. So the buyer must be
//! online and dispute non-delivery before `D`.
//!
//! # Equivocation
//!
//! Both dispute outcomes are valid from height 0. An on-chain delay
//! "after the dispute opens" cannot be expressed: opening a dispute is not
//! a transaction, and every outcome is presigned at funding. A moderator who
//! reveals both halves creates a broadcast race. That moderator also hands out
//! two [`ModeratorRuling`]s it signed for the same outpoint, and
//! [`is_equivocation`] turns those into transferable proof of misconduct.
//!
//! # Moderator fee collection
//!
//! The fee is an output of each dispute outcome, so it is collected only when
//! the winner broadcasts that outcome. A vendor who wins close to `V` can
//! skip it and wait for the fee-free claim. A buyer who wins cannot. That
//! skews moderator pay toward buyer-wins rulings. A fee bond posted at
//! funding is the natural fix and is not implemented here.
//!
//! # Caller obligations
//!
//! * **Segwit funding.** The funding txid is committed in every presigned
//!   spend. Fund from native segwit inputs only. [`FundingPlan::Psbt`]
//!   enforces this. With [`FundingPlan::Outpoint`] it is on the caller.
//! * **Per-order keys.** Derive keys with [`order_secret_key`] /
//!   [`order_public_key`] from a 32-byte secret shared only by the two
//!   parties, and use fresh payout scripts, so orders are unlinkable.
//! * **Persistence.** Store [`SignedRefund`] (buyer) or [`SignedClaim`] (vendor)
//!   before funding or shipping.
//! * **Ship after funding.** The vendor ships only after
//!   [`VendorEscrow::verify_funding`] accepts the confirmed funding transaction.

#![forbid(unsafe_code)]

mod buyer;
mod contract;
mod crypto;
mod error;
mod funding;
#[cfg(feature = "legacy-single-process")]
mod legacy;
mod letter;
mod messages;
mod proofs;
mod terms;
mod tx;
mod vencrypt;
mod vendor;
mod wire;

pub use buyer::BuyerEscrow;
pub use contract::{SignedClaim, SignedRefund};
pub use error::Error;
pub use funding::{
    BuyerAwaitingReply, BuyerSession, FundingPlan, VendorAwaitingSignatures, VendorSession,
};
#[cfg(feature = "legacy-single-process")]
#[allow(deprecated)]
pub use legacy::{fund, FundParams, LegacyEscrowAddress};
pub use letter::{is_equivocation, moderator_open, DisputeCase, ModeratorRuling};
pub use messages::{BuyerOpen, BuyerSignatures, FundingNonces, ReleaseSignature, VendorReply};
pub use proofs::Pok;
pub use terms::{order_public_key, order_secret_key, EscrowTerms, Policy, SignedTerms, DUST_LIMIT};
pub use tx::is_final_at_height;
pub use vencrypt::VerifiableShare;
pub use vendor::{ReleaseNonce, VendorEscrow};

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

/// Explicit escrow state, per party.
///
/// Only `Funded` has outgoing transitions, and each of those is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Funded,
    /// Happy-path release signed (buyer) or assembled (vendor).
    Completed,
    /// Buyer refund broadcast.
    Refunded,
    /// Vendor claim broadcast.
    Claimed,
    /// A dispute outcome was finished from a moderator ruling.
    Disputed,
}
