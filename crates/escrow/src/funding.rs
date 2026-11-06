//! The two-party funding protocol.
//!
//! ```text
//! round 0  both     EscrowTerms::sign            -> SignedTerms
//! round 1  buyer    BuyerSession::start          -> (BuyerAwaitingReply, BuyerOpen)
//! round 2  vendor   VendorSession::accept        -> (VendorAwaitingSignatures, VendorReply)
//! round 3  buyer    BuyerAwaitingReply::receive  -> (BuyerEscrow, BuyerSignatures)
//!          vendor   VendorAwaitingSignatures::receive -> VendorEscrow
//! ```
//!
//! Each state holds exactly one secret key. Secret nonces live only inside a
//! state that is consumed by value and is not `Clone`. The buyer's funding
//! PSBT is moved in at round 1 and only handed back by [`BuyerEscrow`], which
//! exists only once the refund and the buyer-wins adaptor signature have
//! verified against the aggregate key and the exact funding outpoint.
//!
//! Both sides pass a [`Context`]: the current tip, the moderator record they
//! trust, and their own policy. [`Contract`] construction checks all of it.
//!
//! The funding transaction must never be replaced. Every presigned spend
//! commits to its txid, so an RBF bump of the funding transaction strands
//! the refund, claim, and both dispute outcomes. A funding PSBT whose inputs
//! signal replaceability is rejected; with a bare outpoint the caller must
//! guarantee the same.

use bitcoin::{OutPoint, Psbt};
use musig2::secp::{Point, Scalar, G};
use musig2::{AggNonce, PartialSignature, SecNonce};

use crate::buyer::BuyerEscrow;
use crate::context::Context;
use crate::contract::{self, Branch, Contract, Role, SignedClaim, SignedRefund};
use crate::crypto::{self, random_scalar, scalar_key};
use crate::messages::{BuyerOpen, BuyerSignatures, FundingNonces, VendorReply};
use crate::proofs::Pok;
use crate::terms::SignedTerms;
use crate::tx::Spend;
use crate::vencrypt::VerifiableShare;
use crate::vendor::VendorEscrow;
use crate::{Error, Party};

/// How the buyer will fund the escrow output.
///
/// The funding txid is committed into every presigned spend, so it must not
/// change when the funding transaction is signed. Fund only from native
/// segwit inputs (P2WPKH, P2WSH, P2TR) whose sequences do not signal RBF
/// (`nSequence >= 0xfffffffe`), and never fee-bump it by replacement. A
/// `Psbt` is checked for both; with a bare `Outpoint` the caller is
/// responsible for them.
pub enum FundingPlan {
    Psbt(Psbt),
    Outpoint(OutPoint),
}

/// The four secret nonces one party holds between rounds.
struct SecNonces {
    refund: SecNonce,
    claim: SecNonce,
    buyer_wins: SecNonce,
    vendor_wins: SecNonce,
}

fn generate_nonces(
    contract: &Contract,
    secret: Scalar,
    party: Party,
) -> Result<(SecNonces, FundingNonces), Error> {
    let tag = match party {
        Party::Buyer => "buyer",
        Party::Vendor => "vendor",
    };
    let make = |spend: Spend, label: &str| -> Result<SecNonce, Error> {
        let extra = format!("ob-escrow/{label}/{tag}");
        Ok(crypto::fresh_nonce(
            secret,
            &contract.key_agg,
            &contract.sighash(spend)?,
            extra.as_bytes(),
        ))
    };
    let secret_nonces = SecNonces {
        refund: make(Spend::Refund, "refund")?,
        claim: make(Spend::Claim, "claim")?,
        buyer_wins: make(Spend::BuyerWins, "buyer-wins")?,
        vendor_wins: make(Spend::VendorWins, "vendor-wins")?,
    };
    let public = FundingNonces {
        refund: secret_nonces.refund.public_nonce(),
        claim: secret_nonces.claim.public_nonce(),
        buyer_wins: secret_nonces.buyer_wins.public_nonce(),
        vendor_wins: secret_nonces.vendor_wins.public_nonce(),
    };
    Ok((secret_nonces, public))
}

struct AggNonces {
    refund: AggNonce,
    claim: AggNonce,
    buyer_wins: AggNonce,
    vendor_wins: AggNonce,
}

impl AggNonces {
    fn new(buyer: &FundingNonces, vendor: &FundingNonces) -> Self {
        Self {
            refund: crypto::aggregate_nonce(&buyer.refund, &vendor.refund),
            claim: crypto::aggregate_nonce(&buyer.claim, &vendor.claim),
            buyer_wins: crypto::aggregate_nonce(&buyer.buyer_wins, &vendor.buyer_wins),
            vendor_wins: crypto::aggregate_nonce(&buyer.vendor_wins, &vendor.vendor_wins),
        }
    }
}

struct Partials {
    refund: PartialSignature,
    claim: PartialSignature,
    buyer_wins: PartialSignature,
    vendor_wins: PartialSignature,
}

fn sign_all(
    contract: &Contract,
    secret: Scalar,
    nonces: SecNonces,
    agg: &AggNonces,
    buyer_wins_point: Point,
    vendor_wins_point: Point,
) -> Result<Partials, Error> {
    let sign = |nonce, agg, spend, adaptor| {
        crypto::partial_sign(
            &contract.key_agg,
            secret,
            nonce,
            agg,
            &contract.sighash(spend)?,
            adaptor,
        )
    };
    Ok(Partials {
        refund: sign(nonces.refund, &agg.refund, Spend::Refund, None)?,
        claim: sign(nonces.claim, &agg.claim, Spend::Claim, None)?,
        buyer_wins: sign(
            nonces.buyer_wins,
            &agg.buyer_wins,
            Spend::BuyerWins,
            Some(buyer_wins_point),
        )?,
        vendor_wins: sign(
            nonces.vendor_wins,
            &agg.vendor_wins,
            Spend::VendorWins,
            Some(vendor_wins_point),
        )?,
    })
}

fn verify_counterparty_partial(
    contract: &Contract,
    signer: Party,
    partial: PartialSignature,
    agg: &AggNonce,
    pubnonce: &musig2::PubNonce,
    spend: Spend,
    adaptor: Option<Point>,
) -> Result<(), Error> {
    crypto::verify_partial(
        &contract.key_agg,
        partial,
        agg,
        contract.key(signer),
        pubnonce,
        &contract.sighash(spend)?,
        adaptor,
    )
}

fn own_secret(contract: &Contract, secret: &[u8; 32], party: Party) -> Result<Scalar, Error> {
    let scalar = scalar_key(secret)?;
    if scalar * G != contract.key(party) {
        return Err(Error::WrongKey);
    }
    Ok(scalar)
}

/// A moderator half for the branch that favors `favored`, created by the other party.
fn loser_share(contract: &Contract, favored: Party) -> Result<VerifiableShare, Error> {
    VerifiableShare::encrypt(
        random_scalar(),
        &contract.moderator,
        &contract.branch_context(favored)?,
    )
}

fn favored_half(
    contract: &Contract,
    secret: &Scalar,
    favored: Party,
) -> Result<(Point, Pok), Error> {
    let half = contract::party_half(secret, contract, favored)?;
    Ok((
        half * G,
        Pok::prove(half, &contract.branch_context(favored)?),
    ))
}

/// Check a counterparty's favored half and our-side moderator share for one branch.
///
/// The proof of knowledge stops the favored party from choosing its half as
/// `Y - T_mod` for a `Y` it knows, which would let it finish the branch
/// without the moderator.
fn verify_branch_inputs(
    contract: &Contract,
    favored: Party,
    half: &Point,
    proof: &Pok,
    share: &VerifiableShare,
) -> Result<Point, Error> {
    let ctx = contract.branch_context(favored)?;
    proof.verify(half, &ctx)?;
    share.verify(&contract.moderator, &ctx)?;
    contract::adaptor_point(*half, share.point())
}

fn validate_psbt(psbt: &Psbt, funding_output: &bitcoin::TxOut) -> Result<OutPoint, Error> {
    let tx = &psbt.unsigned_tx;
    if tx.input.is_empty() || psbt.inputs.len() != tx.input.len() {
        return Err(Error::BadFunding("PSBT has no inputs"));
    }
    for (txin, input) in tx.input.iter().zip(&psbt.inputs) {
        let segwit = input
            .witness_utxo
            .as_ref()
            .is_some_and(|utxo| utxo.script_pubkey.is_witness_program());
        if !segwit
            || input.redeem_script.is_some()
            || input.final_script_sig.is_some()
            || !txin.script_sig.is_empty()
        {
            return Err(Error::BadFunding(
                "every input must be a native segwit output with a witness_utxo",
            ));
        }
        if txin.sequence.is_rbf() {
            return Err(Error::BadFunding(
                "funding inputs must not signal replace-by-fee",
            ));
        }
    }
    let mut matches = tx
        .output
        .iter()
        .enumerate()
        .filter(|(_, out)| *out == funding_output);
    let (vout, _) = matches
        .next()
        .ok_or(Error::BadFunding("PSBT does not pay the escrow output"))?;
    if matches.next().is_some() {
        return Err(Error::BadFunding("PSBT pays the escrow output twice"));
    }
    Ok(OutPoint {
        txid: tx.compute_txid(),
        vout: vout as u32,
    })
}

/// Buyer entry point.
pub struct BuyerSession;

impl BuyerSession {
    /// Round 1. `buyer_secret` is the buyer's per-order key.
    pub fn start(
        terms: SignedTerms,
        buyer_secret: &[u8; 32],
        ctx: &Context,
        funding: FundingPlan,
    ) -> Result<(BuyerAwaitingReply, BuyerOpen), Error> {
        let (outpoint, psbt) = match funding {
            FundingPlan::Outpoint(outpoint) => (outpoint, None),
            FundingPlan::Psbt(psbt) => {
                let funding_output = terms.terms().funding_output()?;
                (validate_psbt(&psbt, &funding_output)?, Some(psbt))
            }
        };
        let contract = Contract::new(terms, outpoint, ctx, Role::Party)?;
        let secret = own_secret(&contract, buyer_secret, Party::Buyer)?;
        let (nonces, public) = generate_nonces(&contract, secret, Party::Buyer)?;
        let (buyer_half, buyer_half_proof) = favored_half(&contract, &secret, Party::Buyer)?;
        let vendor_wins_share = loser_share(&contract, Party::Vendor)?;
        let message = BuyerOpen {
            funding_outpoint: outpoint,
            nonces: public.clone(),
            buyer_half,
            buyer_half_proof,
            vendor_wins_share: vendor_wins_share.clone(),
        };
        Ok((
            BuyerAwaitingReply {
                contract,
                secret,
                nonces,
                public,
                buyer_half,
                vendor_wins_share,
                psbt,
            },
            message,
        ))
    }
}

/// Buyer after round 1. Holds secret nonces; not `Clone`.
///
/// The funding PSBT cannot be read back from this state:
///
/// ```compile_fail
/// fn leak(state: &openbazaar_escrow::BuyerAwaitingReply) {
///     let _ = state.funding_psbt();
/// }
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<openbazaar_escrow::BuyerAwaitingReply>();
/// ```
pub struct BuyerAwaitingReply {
    contract: Contract,
    secret: Scalar,
    nonces: SecNonces,
    public: FundingNonces,
    buyer_half: Point,
    vendor_wins_share: VerifiableShare,
    psbt: Option<Psbt>,
}

impl BuyerAwaitingReply {
    pub fn funding_outpoint(&self) -> OutPoint {
        self.contract.outpoint
    }

    /// Round 3. Verifies everything the vendor sent before signing anything.
    ///
    /// On error the state, its secret nonces, and the PSBT are dropped.
    pub fn receive(self, reply: &VendorReply) -> Result<(BuyerEscrow, BuyerSignatures), Error> {
        let contract = self.contract;
        let vendor_wins_point = verify_branch_inputs(
            &contract,
            Party::Vendor,
            &reply.vendor_half,
            &reply.vendor_half_proof,
            &self.vendor_wins_share,
        )?;
        let buyer_wins_ctx = contract.branch_context(Party::Buyer)?;
        reply
            .buyer_wins_share
            .verify(&contract.moderator, &buyer_wins_ctx)?;
        let buyer_wins_point =
            contract::adaptor_point(self.buyer_half, reply.buyer_wins_share.point())?;

        let agg = AggNonces::new(&self.public, &reply.nonces);
        verify_counterparty_partial(
            &contract,
            Party::Vendor,
            reply.refund_partial,
            &agg.refund,
            &reply.nonces.refund,
            Spend::Refund,
            None,
        )?;
        verify_counterparty_partial(
            &contract,
            Party::Vendor,
            reply.buyer_wins_partial,
            &agg.buyer_wins,
            &reply.nonces.buyer_wins,
            Spend::BuyerWins,
            Some(buyer_wins_point),
        )?;

        let ours = sign_all(
            &contract,
            self.secret,
            self.nonces,
            &agg,
            buyer_wins_point,
            vendor_wins_point,
        )?;
        let refund_tx = contract.finish_plain(
            Spend::Refund,
            &agg.refund,
            [ours.refund, reply.refund_partial],
        )?;
        let refund = SignedRefund::new(refund_tx, &contract)?;
        let adaptor_sig = crypto::aggregate_adaptor(
            &contract.key_agg,
            &agg.buyer_wins,
            buyer_wins_point,
            [ours.buyer_wins, reply.buyer_wins_partial],
            &contract.sighash(Spend::BuyerWins)?,
        )?;
        let branch = Branch {
            favored: Party::Buyer,
            party_point: self.buyer_half,
            share: reply.buyer_wins_share.clone(),
            adaptor_sig,
        };
        let message = BuyerSignatures {
            claim_partial: ours.claim,
            vendor_wins_partial: ours.vendor_wins,
        };
        let escrow = BuyerEscrow::new(
            contract,
            self.secret,
            refund,
            branch,
            vendor_wins_point,
            self.psbt,
        );
        Ok((escrow, message))
    }
}

/// Vendor entry point.
pub struct VendorSession;

impl VendorSession {
    /// Round 2. `vendor_secret` is the vendor's per-order key.
    ///
    /// The vendor checks the buyer's vendor-wins share and proof before it
    /// signs anything. Its own secret nonces are generated and consumed here.
    pub fn accept(
        terms: SignedTerms,
        vendor_secret: &[u8; 32],
        ctx: &Context,
        open: &BuyerOpen,
    ) -> Result<(VendorAwaitingSignatures, VendorReply), Error> {
        let contract = Contract::new(terms, open.funding_outpoint, ctx, Role::Party)?;
        let secret = own_secret(&contract, vendor_secret, Party::Vendor)?;
        let buyer_ctx = contract.branch_context(Party::Buyer)?;
        open.buyer_half_proof.verify(&open.buyer_half, &buyer_ctx)?;
        let vendor_ctx = contract.branch_context(Party::Vendor)?;
        open.vendor_wins_share
            .verify(&contract.moderator, &vendor_ctx)?;

        let (vendor_half, vendor_half_proof) = favored_half(&contract, &secret, Party::Vendor)?;
        let buyer_wins_share = loser_share(&contract, Party::Buyer)?;
        let buyer_wins_point = contract::adaptor_point(open.buyer_half, buyer_wins_share.point())?;
        let vendor_wins_point =
            contract::adaptor_point(vendor_half, open.vendor_wins_share.point())?;

        let (nonces, public) = generate_nonces(&contract, secret, Party::Vendor)?;
        let agg = AggNonces::new(&open.nonces, &public);
        let ours = sign_all(
            &contract,
            secret,
            nonces,
            &agg,
            buyer_wins_point,
            vendor_wins_point,
        )?;
        let reply = VendorReply {
            nonces: public,
            vendor_half,
            vendor_half_proof,
            buyer_wins_share,
            refund_partial: ours.refund,
            buyer_wins_partial: ours.buyer_wins,
        };
        Ok((
            VendorAwaitingSignatures {
                contract,
                secret,
                agg,
                buyer_nonces: open.nonces.clone(),
                claim_partial: ours.claim,
                vendor_wins_partial: ours.vendor_wins,
                vendor_half,
                vendor_wins_share: open.vendor_wins_share.clone(),
                buyer_wins_point,
            },
            reply,
        ))
    }
}

/// Vendor after round 2. Holds only its own partials, never a secret nonce.
pub struct VendorAwaitingSignatures {
    contract: Contract,
    secret: Scalar,
    agg: AggNonces,
    buyer_nonces: FundingNonces,
    claim_partial: PartialSignature,
    vendor_wins_partial: PartialSignature,
    vendor_half: Point,
    vendor_wins_share: VerifiableShare,
    buyer_wins_point: Point,
}

impl VendorAwaitingSignatures {
    /// Finish funding on the vendor side.
    pub fn receive(self, signatures: &BuyerSignatures) -> Result<VendorEscrow, Error> {
        let contract = self.contract;
        let vendor_wins_point =
            contract::adaptor_point(self.vendor_half, self.vendor_wins_share.point())?;
        verify_counterparty_partial(
            &contract,
            Party::Buyer,
            signatures.claim_partial,
            &self.agg.claim,
            &self.buyer_nonces.claim,
            Spend::Claim,
            None,
        )?;
        verify_counterparty_partial(
            &contract,
            Party::Buyer,
            signatures.vendor_wins_partial,
            &self.agg.vendor_wins,
            &self.buyer_nonces.vendor_wins,
            Spend::VendorWins,
            Some(vendor_wins_point),
        )?;
        let claim_tx = contract.finish_plain(
            Spend::Claim,
            &self.agg.claim,
            [signatures.claim_partial, self.claim_partial],
        )?;
        let claim = SignedClaim::new(claim_tx, &contract)?;
        let adaptor_sig = crypto::aggregate_adaptor(
            &contract.key_agg,
            &self.agg.vendor_wins,
            vendor_wins_point,
            [signatures.vendor_wins_partial, self.vendor_wins_partial],
            &contract.sighash(Spend::VendorWins)?,
        )?;
        let branch = Branch {
            favored: Party::Vendor,
            party_point: self.vendor_half,
            share: self.vendor_wins_share,
            adaptor_sig,
        };
        Ok(VendorEscrow::new(
            contract,
            self.secret,
            claim,
            branch,
            self.buyer_wins_point,
        ))
    }
}
