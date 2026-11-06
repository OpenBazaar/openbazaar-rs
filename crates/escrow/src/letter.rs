//! Dispute letters, moderator rulings, and dispute settlement.
//!
//! The moderator is not a participant at funding. A letter is the first thing
//! it sees, and the letter alone is enough to check every claim in it: the
//! signed terms, the funding script and value, the aggregate key, the exact
//! unsigned outcome transaction, the adaptor pre-signature over its sighash,
//! the verifiable encryption of the moderator half, and the claimant's
//! signature over the whole letter. Only then does the moderator decrypt.

use bitcoin::{OutPoint, Transaction};
use musig2::secp::{Point, Scalar, G};
use musig2::LiftedSignature;
use openbazaar_protocol::{EscrowBranch, EscrowTerms as EnvelopeTerms};

use crate::contract::{self, Branch, Contract};
use crate::crypto::{self, scalar_key};
use crate::proofs::tagged;
use crate::terms::{Policy, SignedTerms};
use crate::tx::{self, Spend};
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer, MEMO_MAX};
use crate::{Error, Party};

const LETTER_MAGIC: &[u8; 4] = b"OBDL";
const LETTER_VERSION: u8 = 2;
const RULING_MAGIC: &[u8; 4] = b"OBRU";
const RULING_VERSION: u8 = 1;

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
    tagged("ob-escrow/letter/v2", &[body])
}

/// A dispute letter whose every field the moderator has verified.
#[derive(Clone, Debug)]
pub struct DisputeCase {
    contract: Contract,
    favored: Party,
    memo: Vec<u8>,
    adaptor_point: Point,
    share: VerifiableShare,
    letter_hash: [u8; 32],
}

impl DisputeCase {
    /// Verify a letter for the moderator holding `moderator_public`.
    ///
    /// `received_height` is the chain height when the letter arrived. Letters
    /// after `dispute_deadline` are refused, so the ruling has the full
    /// ruling window before the vendor claim becomes final.
    pub fn verify(
        letter: &[u8],
        moderator_public: &[u8; 33],
        received_height: u32,
        policy: &Policy,
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

        let contract = Contract::new(signed, outpoint, moderator_public, policy)?;
        if received_height > contract.signed.terms().dispute_deadline {
            return Err(Error::DisputeWindowClosed);
        }
        let body = &letter[..body_len];
        crypto::schnorr_verify(&contract.key(favored), &letter_digest(body), &signature)?;
        if contract.funding_output.value != funding_value
            || contract.funding_output.script_pubkey != funding_script
            || contract.output_key != output_key
        {
            return Err(Error::KeyMismatch);
        }
        let spend = Spend::dispute(favored);
        if &outcome != contract.tx(spend) {
            return Err(Error::BadPayload);
        }
        let sighash = tx::key_spend_sighash(&outcome, &contract.funding_output)?;
        let adaptor_point = contract::adaptor_point(party_point, share.point())?;
        crypto::verify_adaptor(&contract.key_agg, &adaptor_sig, &sighash, adaptor_point)?;
        share.verify(&contract.moderator, &contract.branch_context(favored)?)?;

        Ok(Self {
            contract,
            favored,
            memo,
            adaptor_point,
            share,
            letter_hash: tagged("ob-escrow/letter-bytes/v2", &[letter]),
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

    /// Decrypt the moderator half and sign the ruling.
    pub fn rule(&self, moderator_secret: &[u8; 32]) -> Result<ModeratorRuling, Error> {
        let secret = scalar_key(moderator_secret)?;
        if secret * G != self.contract.moderator {
            return Err(Error::WrongKey);
        }
        let ctx = self.contract.branch_context(self.favored)?;
        let share = self.share.open(secret, &ctx)?;
        let message = ruling_message(&self.contract.outpoint, self.favored, &self.adaptor_point);
        let signature = crypto::schnorr_sign(&secret, &message)?;
        Ok(ModeratorRuling {
            outpoint: self.contract.outpoint,
            favored: self.favored,
            adaptor_point: self.adaptor_point,
            share,
            signature,
        })
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
            && envelope.moderator == terms.moderator
            && envelope.moderator_fee == terms.moderator_fee.to_sat()
            && envelope.dispute_deadline == terms.dispute_deadline
            && envelope.vendor_claim_height == terms.vendor_claim_height
            && envelope.refund_locktime == terms.refund_height
            && claimed.adaptor_point == self.adaptor_point.serialize()
            && tagged("ob-escrow/letter-bytes/v2", &[&claimed.letter]) == self.letter_hash;
        if matches {
            Ok(())
        } else {
            Err(Error::BadPayload)
        }
    }
}

/// Verify a letter and, if it holds up, decrypt and sign a ruling for its branch.
///
/// Calling this is the moderator's decision. It should be called for at most
/// one branch per escrow; see the crate docs on equivocation.
pub fn moderator_open(
    letter: &[u8],
    moderator_secret: &[u8; 32],
    received_height: u32,
    policy: &Policy,
) -> Result<ModeratorRuling, Error> {
    let public = (scalar_key(moderator_secret)? * G).serialize();
    DisputeCase::verify(letter, &public, received_height, policy)?.rule(moderator_secret)
}

fn ruling_message(outpoint: &OutPoint, favored: Party, adaptor_point: &Point) -> [u8; 32] {
    let mut w = Writer::new(b"OBRM", 1);
    w.outpoint(outpoint);
    tagged(
        "ob-escrow/ruling/v1",
        &[w.as_slice(), &[favored as u8], &adaptor_point.serialize()],
    )
}

/// The moderator's decision: its decrypted half, plus a signature naming the branch.
///
/// The half is not a Bitcoin signature. Finishing the outcome also needs the
/// favored party's half. The signature makes the ruling attributable: two
/// valid rulings for the same outpoint and different branches prove the
/// moderator equivocated.
#[derive(Clone)]
pub struct ModeratorRuling {
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

    /// Check the moderator's signature over (outpoint, branch, adaptor point).
    pub fn verify(&self, moderator: &[u8; 33]) -> Result<(), Error> {
        let key = crypto::point_from_compressed(moderator)?;
        let message = ruling_message(&self.outpoint, self.favored, &self.adaptor_point);
        crypto::schnorr_verify(&key, &message, &self.signature)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(RULING_MAGIC, RULING_VERSION);
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

/// Two rulings that prove the moderator signed both outcomes of one escrow.
pub fn is_equivocation(a: &ModeratorRuling, b: &ModeratorRuling, moderator: &[u8; 33]) -> bool {
    a.outpoint == b.outpoint
        && a.favored != b.favored
        && a.verify(moderator).is_ok()
        && b.verify(moderator).is_ok()
}

/// Finish the favored party's dispute outcome from the moderator's ruling.
pub(crate) fn settle(
    contract: &Contract,
    branch: &Branch,
    secret: &Scalar,
    ruling: &ModeratorRuling,
) -> Result<Transaction, Error> {
    ruling.verify(&contract.signed.terms().moderator)?;
    let adaptor_point = branch.adaptor_point()?;
    if ruling.outpoint != contract.outpoint
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
        moderator: terms.moderator,
        moderator_fee: terms.moderator_fee.to_sat(),
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
