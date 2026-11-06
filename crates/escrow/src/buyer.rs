//! The buyer's side of a funded escrow.

use bitcoin::{OutPoint, Psbt, Transaction, TxOut};
use musig2::secp::{Point, Scalar};
use musig2::PubNonce;
use openbazaar_protocol::EscrowTerms as EnvelopeTerms;

use crate::contract::{self, Branch, Contract, SignedRefund};
use crate::crypto;
use crate::letter::{self, ModeratorRuling};
use crate::messages::ReleaseSignature;
use crate::tx::Spend;
use crate::{Error, EscrowTerms, Party, Phase};

/// Buyer state after funding round 3.
///
/// Holding one means the refund is fully signed and verified, and the
/// buyer-wins adaptor signature and its moderator share have verified. Only
/// now is the funding PSBT handed back for signing.
///
/// Not `Clone`: a copy could refund after its original signed the release,
/// or settle a dispute twice.
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<openbazaar_escrow::BuyerEscrow>();
/// ```
pub struct BuyerEscrow {
    contract: Contract,
    secret: Scalar,
    refund: SignedRefund,
    branch: Branch,
    vendor_wins_point: Point,
    psbt: Option<Psbt>,
    phase: Phase,
}

impl core::fmt::Debug for BuyerEscrow {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BuyerEscrow")
            .field("outpoint", &self.contract.outpoint)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl BuyerEscrow {
    pub(crate) fn new(
        contract: Contract,
        secret: Scalar,
        refund: SignedRefund,
        branch: Branch,
        vendor_wins_point: Point,
        psbt: Option<Psbt>,
    ) -> Self {
        Self {
            contract,
            secret,
            refund,
            branch,
            vendor_wins_point,
            psbt,
            phase: Phase::Funded,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn terms(&self) -> &EscrowTerms {
        self.contract.signed.terms()
    }

    pub fn funding_outpoint(&self) -> OutPoint {
        self.contract.outpoint
    }

    pub fn funding_output(&self) -> &TxOut {
        &self.contract.funding_output
    }

    /// Tweaked MuSig2 x-only key. Every valid spend verifies under this key.
    pub fn output_key(&self) -> [u8; 32] {
        self.contract.output_key
    }

    /// The unsigned funding PSBT passed to round 1, now safe to sign.
    pub fn funding_psbt(&self) -> Option<&Psbt> {
        self.psbt.as_ref()
    }

    /// The persistable refund. Store it before broadcasting the funding transaction.
    pub fn refund(&self) -> &SignedRefund {
        &self.refund
    }

    /// Unsigned dispute outcome for `favored`, as both parties agreed it.
    pub fn dispute_outcome(&self, favored: Party) -> &Transaction {
        self.contract.tx(Spend::dispute(favored))
    }

    /// BIP 341 key-spend check against the funding output. Does not apply `nLockTime`.
    pub fn verify_key_spend(&self, tx: &Transaction) -> Result<(), Error> {
        self.contract.verify_key_spend(tx)
    }

    /// Broadcast the refund. Fails without changing phase before `refund_height + 1`.
    pub fn broadcast_refund(&mut self, height: u32) -> Result<Transaction, Error> {
        let tx = self.refund.broadcastable_at(height)?;
        contract::enter(&mut self.phase, Phase::Refunded)?;
        Ok(tx)
    }

    /// Sign the happy-path release to the vendor, given the vendor's release nonce.
    ///
    /// The buyer generates its nonce here and signs in the same step, so no
    /// buyer secret nonce outlives this call.
    ///
    /// The buyer may sign again (for example after the vendor lost its
    /// nonce) until the release confirms. Once signed, the refund and dispute
    /// paths are closed on this side.
    pub fn sign_release(&mut self, vendor_nonce: &PubNonce) -> Result<ReleaseSignature, Error> {
        if !matches!(self.phase, Phase::Funded | Phase::ReleaseSigned) {
            return Err(Error::InvalidTransition {
                from: self.phase,
                to: Phase::ReleaseSigned,
            });
        }
        let sighash = self.contract.sighash(Spend::Release)?;
        let secnonce = crypto::fresh_nonce(
            self.secret,
            &self.contract.key_agg,
            &sighash,
            b"ob-escrow/release/buyer",
        );
        let nonce = secnonce.public_nonce();
        let agg = crypto::aggregate_nonce(&nonce, vendor_nonce);
        let partial = crypto::partial_sign(
            &self.contract.key_agg,
            self.secret,
            secnonce,
            &agg,
            &sighash,
            None,
        )?;
        self.phase = Phase::ReleaseSigned;
        Ok(ReleaseSignature { nonce, partial })
    }

    /// A signed letter asking the moderator to rule for the buyer.
    pub fn dispute_letter(&self, memo: &[u8]) -> Result<Vec<u8>, Error> {
        letter::build_letter(&self.contract, &self.branch, &self.secret, memo)
    }

    /// The protocol envelope's escrow summary, carrying `letter` on the buyer-wins branch.
    pub fn envelope_terms(&self, letter: Vec<u8>) -> Result<EnvelopeTerms, Error> {
        Ok(letter::envelope_terms(
            &self.contract,
            Party::Buyer,
            self.branch.adaptor_point()?,
            self.vendor_wins_point,
            letter,
        ))
    }

    /// Finish the buyer-wins outcome with the moderator's ruling.
    pub fn settle_dispute(&mut self, ruling: &ModeratorRuling) -> Result<Transaction, Error> {
        if self.phase != Phase::Funded {
            return Err(Error::InvalidTransition {
                from: self.phase,
                to: Phase::Disputed,
            });
        }
        let tx = letter::settle(&self.contract, &self.branch, &self.secret, ruling)?;
        contract::enter(&mut self.phase, Phase::Disputed)?;
        Ok(tx)
    }

    /// A copy for tests that start many scenarios from one funded escrow.
    #[cfg(test)]
    pub(crate) fn duplicate(&self) -> Self {
        Self {
            contract: self.contract.clone(),
            secret: self.secret,
            refund: self.refund.clone(),
            branch: self.branch.clone(),
            vendor_wins_point: self.vendor_wins_point,
            psbt: self.psbt.clone(),
            phase: self.phase,
        }
    }

    #[cfg(test)]
    pub(crate) fn branch(&self) -> &Branch {
        &self.branch
    }

    /// A letter the buyer signs over a doctored branch.
    #[cfg(test)]
    pub(crate) fn forged_letter(&self, edit: impl FnOnce(&mut Branch)) -> Vec<u8> {
        let mut branch = self.branch.clone();
        edit(&mut branch);
        letter::build_letter(&self.contract, &branch, &self.secret, b"forged").unwrap()
    }
}
