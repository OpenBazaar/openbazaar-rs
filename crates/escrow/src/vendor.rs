//! The vendor's side of a funded escrow.

use bitcoin::{OutPoint, Transaction, TxOut};
use musig2::secp::{Point, Scalar};
use musig2::{PubNonce, SecNonce};
use openbazaar_protocol::EscrowTerms as EnvelopeTerms;

use crate::contract::{self, Branch, Contract, SignedClaim};
use crate::crypto;
use crate::letter::{self, ModeratorRuling};
use crate::messages::ReleaseSignature;
use crate::tx::Spend;
use crate::{Error, EscrowTerms, Party, Phase};

/// Vendor state after funding round 3.
///
/// Holding one means the vendor claim is fully signed and verified, and the
/// vendor-wins adaptor signature and its moderator share have verified. Ship
/// only after [`VendorEscrow::verify_funding`] accepts the confirmed funding
/// transaction.
///
/// Not `Clone`, so a copy cannot claim after its original settled a dispute.
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<openbazaar_escrow::VendorEscrow>();
/// ```
pub struct VendorEscrow {
    contract: Contract,
    secret: Scalar,
    claim: SignedClaim,
    branch: Branch,
    buyer_wins_point: Point,
    phase: Phase,
}

/// The vendor's first release message. Holds a secret nonce; not `Clone`.
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<openbazaar_escrow::ReleaseNonce>();
/// ```
pub struct ReleaseNonce {
    secnonce: SecNonce,
    nonce: PubNonce,
}

impl ReleaseNonce {
    /// Send this to the buyer.
    pub fn public(&self) -> &PubNonce {
        &self.nonce
    }
}

impl core::fmt::Debug for VendorEscrow {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VendorEscrow")
            .field("outpoint", &self.contract.outpoint)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl VendorEscrow {
    pub(crate) fn new(
        contract: Contract,
        secret: Scalar,
        claim: SignedClaim,
        branch: Branch,
        buyer_wins_point: Point,
    ) -> Self {
        Self {
            contract,
            secret,
            claim,
            branch,
            buyer_wins_point,
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

    pub fn output_key(&self) -> [u8; 32] {
        self.contract.output_key
    }

    /// Check that `tx` creates the escrow output at the agreed outpoint.
    pub fn verify_funding(&self, tx: &Transaction) -> Result<(), Error> {
        let outpoint = self.contract.outpoint;
        let paid = tx.output.get(outpoint.vout as usize);
        if tx.compute_txid() != outpoint.txid || paid != Some(&self.contract.funding_output) {
            return Err(Error::BadFunding(
                "transaction does not create the escrow output",
            ));
        }
        Ok(())
    }

    /// The persistable vendor claim.
    pub fn claim(&self) -> &SignedClaim {
        &self.claim
    }

    pub fn verify_key_spend(&self, tx: &Transaction) -> Result<(), Error> {
        self.contract.verify_key_spend(tx)
    }

    /// Broadcast the vendor claim. Fails without changing phase before `vendor_claim_height + 1`.
    pub fn broadcast_claim(&mut self, height: u32) -> Result<Transaction, Error> {
        let tx = self.claim.broadcastable_at(height)?;
        contract::enter(&mut self.phase, Phase::Claimed)?;
        Ok(tx)
    }

    /// First message of the happy-path release.
    pub fn release_nonce(&self) -> Result<ReleaseNonce, Error> {
        let sighash = self.contract.sighash(Spend::Release)?;
        let secnonce = crypto::fresh_nonce(
            self.secret,
            &self.contract.key_agg,
            &sighash,
            b"ob-escrow/release/vendor",
        );
        let nonce = secnonce.public_nonce();
        Ok(ReleaseNonce { secnonce, nonce })
    }

    /// Combine the buyer's release signature with ours into a broadcastable release.
    pub fn complete_release(
        &mut self,
        nonce: ReleaseNonce,
        buyer: &ReleaseSignature,
    ) -> Result<Transaction, Error> {
        if self.phase != Phase::Funded {
            return Err(Error::InvalidTransition {
                from: self.phase,
                to: Phase::Completed,
            });
        }
        let sighash = self.contract.sighash(Spend::Release)?;
        let agg = crypto::aggregate_nonce(&buyer.nonce, &nonce.nonce);
        crypto::verify_partial(
            &self.contract.key_agg,
            buyer.partial,
            &agg,
            self.contract.buyer,
            &buyer.nonce,
            &sighash,
            None,
        )?;
        let ours = crypto::partial_sign(
            &self.contract.key_agg,
            self.secret,
            nonce.secnonce,
            &agg,
            &sighash,
            None,
        )?;
        let tx = self
            .contract
            .finish_plain(Spend::Release, &agg, [buyer.partial, ours])?;
        contract::enter(&mut self.phase, Phase::Completed)?;
        Ok(tx)
    }

    /// A signed letter asking the moderator to rule for the vendor.
    pub fn dispute_letter(&self, memo: &[u8]) -> Result<Vec<u8>, Error> {
        letter::build_letter(&self.contract, &self.branch, &self.secret, memo)
    }

    /// The protocol envelope's escrow summary, carrying `letter` on the vendor-wins branch.
    pub fn envelope_terms(&self, letter: Vec<u8>) -> Result<EnvelopeTerms, Error> {
        Ok(letter::envelope_terms(
            &self.contract,
            Party::Vendor,
            self.branch.adaptor_point()?,
            self.buyer_wins_point,
            letter,
        ))
    }

    /// A copy for tests that start many scenarios from one funded escrow.
    #[cfg(test)]
    pub(crate) fn duplicate(&self) -> Self {
        Self {
            contract: self.contract.clone(),
            secret: self.secret,
            claim: self.claim.clone(),
            branch: self.branch.clone(),
            buyer_wins_point: self.buyer_wins_point,
            phase: self.phase,
        }
    }

    /// Finish the vendor-wins outcome with the moderator's ruling.
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
}
