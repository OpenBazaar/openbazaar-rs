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
//! | spend          | `nLockTime`           | pays                              | moderator |
//! |----------------|-----------------------|-----------------------------------|-----------|
//! | release        | 0                     | vendor                            | no        |
//! | vendor claim   | `refund_height`       | buyer + anchor (the refund)       | no        |
//! | buyer refund   | `refund_height`       | buyer + anchor                    | no        |
//! | buyer wins     | 0                     | buyer + moderator fee + anchor    | picks it  |
//! | vendor wins    | 0                     | vendor + moderator fee + anchor   | picks it  |
//!
//! The anchor is a 240-sat pay-to-anchor output ([`ANCHOR_VALUE`]) so the
//! recipient can CPFP a presigned spend whose fee was fixed at funding.
//!
//! # Context
//!
//! The buyer, the vendor, and the moderator each build the contract from a
//! [`Context`]: the chain tip, the moderator's published [`ModeratorRecord`]
//! (signing key, encryption key, payout script, fee, policy), and their own
//! [`Policy`]. The terms must carry that exact record and pass both policies,
//! and every payout must be a standard script above its dust limit. Because
//! the moderator checks letters against the policy in its own record, terms
//! that funded cannot be refused later by a private moderator policy.
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
//! default). At funding, `D >= tip + min_delivery_window` and
//! `R <= tip + max_refund_horizon`. A transaction with `nLockTime = h` can
//! first be mined at `h + 1`.
//!
//! ```text
//! funding ──── ship, deliver ──── D ── ruling window ── V ── claim window ── R ──▶
//!   │                              │                     │                    │
//!   │ release any time             │ last height a       │ ruling window ends │ buyer refund
//!   │ dispute letters accepted ────┘ letter is accepted  │ no vendor payout   │ minable at R + 1
//!   │ dispute outcomes valid at any height once ruled    │                    │
//! ```
//!
//! * Before `D`, a buyer who did not get the goods opens a dispute. The
//!   moderator must rule, and the winner must confirm, before the refund.
//! * A silent moderator does not pay the vendor. The buyer refund at `R` is
//!   that outcome. The presigned claim is the same refund.
//! * The cooperative release still pays the vendor, with no moderator fee.
//!
//! Silence is the buyer refund. A vendor who should be paid asks for a
//! ruling; a ruling for the vendor pays the vendor. A second presigned set
//! after a shipping attestation would not help: a buyer who wants to cheat
//! would simply not sign it, and without a third-party oracle a shipping
//! attestation is not a cryptographic event.
//!
//! # Equivocation
//!
//! Both dispute outcomes are valid from height 0. An on-chain delay
//! "after the dispute opens" cannot be expressed: opening a dispute is not
//! a transaction, and every outcome is presigned at funding. A moderator who
//! reveals both halves creates a broadcast race. That moderator also hands out
//! two [`ModeratorRuling`]s it signed for the same terms digest and outpoint,
//! and [`is_equivocation`] turns those into transferable proof of misconduct.
//!
//! An honest moderator rules only through [`Moderator::open`], which checks
//! the funding output against its own [`FundingChain`] and records the branch
//! in a [`RulingStore`] before decrypting. A second letter for the other
//! branch of the same escrow gets [`Error::AlreadyRuled`]. The store must be
//! durable: an in-memory store forgets rulings on restart.
//!
//! # Moderator fee collection
//!
//! The fee is an output of a published ruling, buyer-wins or vendor-wins.
//! It is not an output of the presigned claim. Silence follows the buyer
//! refund, so a missing moderator pays neither the vendor nor the fee.
//! A ruling for the vendor still pays the vendor and the fee. The cooperative
//! release and the buyer refund are fee-free.
//!
//! # Caller obligations
//!
//! * **Segwit, non-replaceable funding.** The funding txid is committed in
//!   every presigned spend. Fund from native segwit inputs whose sequences do
//!   not signal RBF, and never replace the funding transaction: a fee-bumped
//!   replacement has a new txid and strands every presigned spend. Bump with
//!   CPFP instead. [`FundingPlan::Psbt`] enforces the input rules. With
//!   [`FundingPlan::Outpoint`] they are on the caller.
//! * **Fees.** Presigned spends carry a fee fixed at funding, at least
//!   [`Policy::min_fee_rate_sat_vb`]. A ruled outcome and the later refund
//!   can race; the winner should CPFP through the anchor at once.
//! * **Moderator chain view and store.** [`FundingChain`] must answer from a
//!   node the moderator runs, and [`RulingStore`] must persist atomically.
//! * **Per-order keys.** Derive keys with [`order_secret_key`] /
//!   [`order_public_key`] from a 32-byte secret shared only by the two
//!   parties, and use fresh payout scripts, so orders are unlinkable.
//! * **Persistence.** Store [`SignedRefund`] (buyer) or [`SignedClaim`] (vendor)
//!   before funding or shipping. [`BuyerEscrow::snapshot`],
//!   [`VendorEscrow::snapshot`], and [`VendorAwaitingSignatures::snapshot`]
//!   hold everything else a restart needs except the per-order secret, which
//!   `restore` takes back. Snapshots never hold a secret nonce, so
//!   [`BuyerAwaitingReply`] cannot be stored: after a restart in round 1 the
//!   buyer starts round 1 again with fresh nonces. A restored state is a
//!   copy: store a new snapshot after every phase change and before sending
//!   or broadcasting its result, and never act on two copies.
//! * **Ship after funding.** The vendor ships only after
//!   [`VendorEscrow::verify_funding`] accepts the confirmed funding transaction.

#![forbid(unsafe_code)]

mod buyer;
mod context;
mod contract;
mod crypto;
mod error;
mod funding;
mod letter;
mod messages;
mod moderator;
mod policy;
mod profile;
mod proofs;
mod public_ruling;
mod state;
mod terms;
mod tx;
mod vencrypt;
mod vendor;
mod wire;

pub use buyer::BuyerEscrow;
pub use context::Context;
pub use contract::{SignedClaim, SignedRefund};
pub use error::Error;
pub use funding::{
    BuyerAwaitingReply, BuyerSession, FundingPlan, VendorAwaitingSignatures, VendorSession,
};
pub use letter::{is_equivocation, DisputeCase, ModeratorRuling};
pub use messages::{BuyerOpen, BuyerSignatures, FundingNonces, ReleaseSignature, VendorReply};
pub use moderator::{
    review_letter, FundingChain, InMemoryRulingStore, LetterCache, Moderator, ModeratorKeys,
    ModeratorRecord, RulingKey, RulingStore,
};
pub use policy::{payout_dust_limit, Policy, ANCHOR_VALUE};
pub use profile::{read_profile, VerifiedProfile};
pub use public_ruling::{read_published_ruling, PublishedSale};
pub use proofs::Pok;
pub use terms::{order_public_key, order_secret_key, EscrowTerms, SignedTerms};
pub use tx::{anchor_script, is_final_at_height, max_spend_vsize};
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
/// Only `Funded` has outgoing transitions. `ReleaseSigned` may only re-sign
/// the release; every other phase is terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Funded,
    /// Buyer signed the happy-path release. It may sign again (fresh nonces)
    /// until the release confirms; refund and dispute are closed.
    ReleaseSigned,
    /// Happy-path release assembled (vendor).
    Completed,
    /// Buyer refund broadcast.
    Refunded,
    /// Vendor claim broadcast.
    Claimed,
    /// A dispute outcome was finished from a moderator ruling.
    Disputed,
}
