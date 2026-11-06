//! Dispute letters, moderator rulings, and dispute settlement.
//!
//! The moderator is not a participant at funding. A letter is the first thing
//! it sees, and the letter alone is enough to check every claim in it: the
//! signed terms, the funding script and value, the aggregate key, the exact
//! unsigned outcome transaction, the adaptor pre-signature over its sighash,
//! the verifiable encryption of the moderator half, and the claimant's
//! signature over the whole letter. The funding output is also checked
//! against the moderator's own chain view. Only then does the moderator
//! decrypt, through [`crate::Moderator::open`].

use bitcoin::{OutPoint, Transaction};
use musig2::secp::{Point, Scalar, G};
use musig2::LiftedSignature;
use openbazaar_protocol::{EscrowBranch, EscrowTerms as EnvelopeTerms};

use crate::context::Context;
use crate::contract::{self, Branch, Contract, Role};
use crate::crypto;
use crate::moderator::{FundingChain, ModeratorKeys, RulingKey};
use crate::proofs::tagged;
use crate::terms::SignedTerms;
use crate::tx::{self, Spend};
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer, MEMO_MAX};
use crate::{Error, Party};

const LETTER_MAGIC: &[u8; 4] = b"OBDL";
const LETTER_VERSION: u8 = 3;
const RULING_MAGIC: &[u8; 4] = b"OBRU";
const RULING_VERSION: u8 = 2;

/// Build and sign a letter asking the moderator to rule for `branch.favored`.
pub(crate) fn build_letter(
    contract: &Contract,
    branch: &Branch,
    secret: &Scalar,
    memo: &[u8],
) -> Result<Vec<u8>, Error> {
    if memo.len() > MEMO_MAX {
        return Err(Error::InvalidParams("claim memo is too long"));
    }
    let mut w = Writer::new(LETTER_MAGIC, LETTER_VERSION);
    w.var(&contract.signed.encode());
    w.outpoint(&contract.outpoint);
    w.amount(contract.funding_output.value);
    w.script(&contract.funding_output.script_pubkey);
    w.bytes(&contract.output_key);
    w.u8(branch.favored as u8);
    w.tx(contract.tx(Spend::dispute(branch.favored)));
    w.point(&branch.party_point);
    branch.share.write(&mut w);
    w.adaptor(&branch.adaptor_sig);
    w.var(memo);
    let signature = crypto::schnorr_sign(secret, &letter_digest(w.as_slice()))?;
    w.bytes(&signature);
    Ok(w.finish())
}

fn letter_digest(body: &[u8]) -> [u8; 32] {
    tagged("ob-escrow/letter/v3", &[body])
}

/// A dispute letter whose every field the moderator has verified.
#[derive(Debug)]
pub struct DisputeCase {
    contract: Contract,
    favored: Party,
    memo: Vec<u8>,
    adaptor_point: Point,
    share: VerifiableShare,
    letter_hash: [u8; 32],
}

impl DisputeCase {
    pub(crate) fn letter_hash(letter: &[u8]) -> [u8; 32] {
        tagged("ob-escrow/letter-bytes/v3", &[letter])
    }

    /// Verify a letter against the moderator context and chain view.
    ///
    /// `ctx.tip` is the height the letter arrived; letters after
    /// `dispute_deadline` are refused. `ctx.moderator` must be exactly the
    /// record in the terms, and the terms are checked against the policy in
    /// that record, so a moderator cannot refuse terms it published.
    ///
    /// `proofs_cached` skips only the adaptor and share proofs, for letter
    /// bytes whose hash already passed them.
    pub(crate) fn verify(
        letter: &[u8],
        ctx: &Context,
        chain: &impl FundingChain,
        proofs_cached: bool,
    ) -> Result<Self, Error> {
        let mut r = Reader::new(letter, LETTER_MAGIC, LETTER_VERSION)?;
        let signed = SignedTerms::decode(r.var(usize::MAX)?)?;
        let outpoint = r.outpoint()?;
        let funding_value = r.amount()?;
        let funding_script = r.script()?;
        let output_key: [u8; 32] = r.array()?;
        let favored = Party::from_byte(r.u8()?).ok_or(Error::BadPayload)?;
        let outcome = r.tx()?;
        let party_point = r.point()?;
        let share = VerifiableShare::read(&mut r)?;
        let adaptor_sig = r.adaptor()?;
        let memo = r.var(MEMO_MAX)?.to_vec();
        let body_len = r.position();
        let signature: [u8; 64] = r.array()?;
        r.finish()?;

        let contract = Contract::new(signed, outpoint, ctx, Role::Moderator)?;
        let body = &letter[..body_len];
        crypto::schnorr_verify(&contract.key(favored), &letter_digest(body), &signature)?;
        if contract.funding_output.value != funding_value
            || contract.funding_output.script_pubkey != funding_script
            || contract.output_key != output_key
        {
            return Err(Error::KeyMismatch);
        }
        if chain.funding_output(&outpoint).as_ref() != Some(&contract.funding_output) {
            return Err(Error::FundingNotOnChain);
        }
        let spend = Spend::dispute(favored);
        if &outcome != contract.tx(spend) {
            return Err(Error::BadPayload);
        }
        let adaptor_point = contract::adaptor_point(party_point, share.point())?;
        if !proofs_cached {
            let sighash = tx::key_spend_sighash(&outcome, &contract.funding_output)?;
            crypto::verify_adaptor(&contract.key_agg, &adaptor_sig, &sighash, adaptor_point)?;
            share.verify(&contract.moderator, &contract.branch_context(favored)?)?;
        }

        Ok(Self {
            contract,
            favored,
            memo,
            adaptor_point,
            share,
            letter_hash: Self::letter_hash(letter),
        })
    }

    pub fn favored(&self) -> Party {
        self.favored
    }

    pub fn memo(&self) -> &[u8] {
        &self.memo
    }

    pub fn terms(&self) -> &crate::EscrowTerms {
        self.contract.signed.terms()
    }

    pub fn funding_outpoint(&self) -> OutPoint {
        self.contract.outpoint
    }

    /// The unsigned outcome the ruling would unlock, including the moderator fee output.
    pub fn outcome(&self) -> &Transaction {
        self.contract.tx(Spend::dispute(self.favored))
    }

    pub(crate) fn ruling_key(&self) -> RulingKey {
        RulingKey {
            terms_digest: self.terms().digest(),
            outpoint: self.contract.outpoint,
        }
    }

    /// Decrypt the moderator half and sign the ruling. The share was verified
    /// in [`Self::verify`]; the decrypted half is checked against its point.
    pub(crate) fn rule(&self, keys: &ModeratorKeys) -> Result<ModeratorRuling, Error> {
        if *keys.decryption() * G != self.contract.moderator {
            return Err(Error::WrongKey);
        }
        let share = self.share.open(*keys.decryption())?;
        let mut ruling = ModeratorRuling {
            terms_digest: self.terms().digest(),
            output_key: self.contract.output_key,
            outpoint: self.contract.outpoint,
            favored: self.favored,
            adaptor_point: self.adaptor_point,
            share,
            signature: [0; 64],
        };
        ruling.signature = crypto::schnorr_sign(keys.signing(), &ruling.message())?;
        Ok(ruling)
    }

    /// Check that a protocol dispute envelope's escrow summary matches this letter.
    pub fn check_envelope(&self, envelope: &EnvelopeTerms) -> Result<(), Error> {
        let terms = self.terms();
        let claimed = match self.favored {
            Party::Buyer => &envelope.buyer_wins,
            Party::Vendor => &envelope.vendor_wins,
        };
        let matches = envelope.buyer == self.contract.buyer.serialize_xonly()
            && envelope.vendor == self.contract.vendor.serialize_xonly()
            && envelope.moderator == terms.moderator.signing_key
            && envelope.moderator_fee == terms.moderator.fee.to_sat()
            && envelope.dispute_deadline == terms.dispute_deadline
            && envelope.vendor_claim_height == terms.vendor_claim_height
            && envelope.refund_locktime == terms.refund_height
            && claimed.adaptor_point == self.adaptor_point.serialize()
            && Self::letter_hash(&claimed.letter) == self.letter_hash;
        if matches {
            Ok(())
        } else {
            Err(Error::BadPayload)
        }
    }
}

/// The moderator's decision: its decrypted half, plus a signature naming the
/// escrow (terms digest, funding output key, outpoint) and the branch.
///
/// The half is not a Bitcoin signature. Finishing the outcome also needs the
/// favored party's half. The signature makes the ruling attributable: two
/// valid rulings for the same terms and outpoint and different branches prove
/// the moderator equivocated.
#[derive(Clone)]
pub struct ModeratorRuling {
    terms_digest: [u8; 32],
    output_key: [u8; 32],
    outpoint: OutPoint,
    favored: Party,
    adaptor_point: Point,
    share: Scalar,
    signature: [u8; 64],
}

impl core::fmt::Debug for ModeratorRuling {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModeratorRuling")
            .field("outpoint", &self.outpoint)
            .field("favored", &self.favored)
            .finish_non_exhaustive()
    }
}

impl ModeratorRuling {
    pub fn favored(&self) -> Party {
        self.favored
    }

    pub fn outpoint(&self) -> OutPoint {
        self.outpoint
    }

    pub fn terms_digest(&self) -> [u8; 32] {
        self.terms_digest
    }

    fn message(&self) -> [u8; 32] {
        let mut w = Writer::new(b"OBRM", 2);
        w.bytes(&self.terms_digest);
        w.bytes(&self.output_key);
        w.outpoint(&self.outpoint);
        w.u8(self.favored as u8);
        w.point(&self.adaptor_point);
        tagged("ob-escrow/ruling/v2", &[w.as_slice()])
    }

    /// Check the moderator's signature with its published signing key.
    pub fn verify(&self, moderator_signing_key: &[u8; 33]) -> Result<(), Error> {
        let key = crypto::point_from_compressed(moderator_signing_key)?;
        crypto::schnorr_verify(&key, &self.message(), &self.signature)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(RULING_MAGIC, RULING_VERSION);
        w.bytes(&self.terms_digest);
        w.bytes(&self.output_key);
        w.outpoint(&self.outpoint);
        w.u8(self.favored as u8);
        w.point(&self.adaptor_point);
        w.bytes(&self.share.serialize());
        w.bytes(&self.signature);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, RULING_MAGIC, RULING_VERSION)?;
        let ruling = Self {
            terms_digest: r.array()?,
            output_key: r.array()?,
            outpoint: r.outpoint()?,
            favored: Party::from_byte(r.u8()?).ok_or(Error::Malformed)?,
            adaptor_point: r.point()?,
            share: Scalar::from_slice(r.take(32)?).map_err(|_| Error::Malformed)?,
            signature: r.array()?,
        };
        r.finish()?;
        Ok(ruling)
    }

    #[cfg(test)]
    pub(crate) fn share(&self) -> Scalar {
        self.share
    }
}

/// Two rulings that prove the moderator signed both outcomes of one escrow:
/// same terms digest, output key, and outpoint, different branches.
///
/// Rulings over different terms are never equivocation, so a party cannot
/// frame the moderator by getting it to rule on terms it re-signed.
pub fn is_equivocation(
    a: &ModeratorRuling,
    b: &ModeratorRuling,
    moderator_signing_key: &[u8; 33],
) -> bool {
    a.terms_digest == b.terms_digest
        && a.output_key == b.output_key
        && a.outpoint == b.outpoint
        && a.favored != b.favored
        && a.verify(moderator_signing_key).is_ok()
        && b.verify(moderator_signing_key).is_ok()
}

/// Finish the favored party's dispute outcome from the moderator's ruling.
pub(crate) fn settle(
    contract: &Contract,
    branch: &Branch,
    secret: &Scalar,
    ruling: &ModeratorRuling,
) -> Result<Transaction, Error> {
    let terms = contract.signed.terms();
    ruling.verify(&terms.moderator.signing_key)?;
    let adaptor_point = branch.adaptor_point()?;
    if ruling.terms_digest != terms.digest()
        || ruling.output_key != contract.output_key
        || ruling.outpoint != contract.outpoint
        || ruling.favored != branch.favored
        || ruling.adaptor_point != adaptor_point
    {
        return Err(Error::BadPayload);
    }
    if ruling.share * G != branch.share.point() {
        return Err(Error::BadProof);
    }
    let half = contract::party_half(secret, contract, branch.favored)?;
    if half * G != branch.party_point {
        return Err(Error::WrongKey);
    }
    let full = (half + ruling.share)
        .not_zero()
        .map_err(|_| Error::BadSignature)?;
    let lifted: LiftedSignature = branch.adaptor_sig.adapt(full).ok_or(Error::BadSignature)?;
    let tx = tx::attach_key_spend(
        contract.tx(Spend::dispute(branch.favored)).clone(),
        &lifted.serialize(),
    );
    contract.verify_key_spend(&tx)?;
    Ok(tx)
}

/// The protocol-level escrow summary for a dispute envelope.
pub(crate) fn envelope_terms(
    contract: &Contract,
    claimed: Party,
    claimed_point: Point,
    other_point: Point,
    letter: Vec<u8>,
) -> EnvelopeTerms {
    let terms = contract.signed.terms();
    let (buyer_point, vendor_point) = match claimed {
        Party::Buyer => (claimed_point, other_point),
        Party::Vendor => (other_point, claimed_point),
    };
    let (buyer_letter, vendor_letter) = match claimed {
        Party::Buyer => (letter, Vec::new()),
        Party::Vendor => (Vec::new(), letter),
    };
    EnvelopeTerms {
        buyer: contract.buyer.serialize_xonly(),
        vendor: contract.vendor.serialize_xonly(),
        moderator: terms.moderator.signing_key,
        moderator_fee: terms.moderator.fee.to_sat(),
        dispute_deadline: terms.dispute_deadline,
        vendor_claim_height: terms.vendor_claim_height,
        refund_locktime: terms.refund_height,
        buyer_wins: EscrowBranch {
            adaptor_point: buyer_point.serialize(),
            letter: buyer_letter,
        },
        vendor_wins: EscrowBranch {
            adaptor_point: vendor_point.serialize(),
            letter: vendor_letter,
        },
    }
}
