//! Signing and checking the filing-fee records of a v5 case.
//!
//! Every record names the case id, a tagged hash of the terms digest and the
//! funding outpoint, so a request, receipt, or decision for one escrow never
//! verifies for another. A filer accepts a request only if the terms it
//! holds name the signer, fix the amount, and make it the filer.

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Keypair, Secp256k1, SecretKey};
use bitcoin::{Amount, OutPoint, ScriptBuf};
use openbazaar_protocol::{
    ruling_digest, CaseAccepted, DisputeDecision, FeeReceipt, FeeRequest, Signed, SignedCaseAccepted,
    SignedDisputeDecision, SignedFeeReceipt, SignedFeeRequest, Terms, DECISION_BUYER, DECISION_DENIED_BUYER,
    DECISION_DENIED_VENDOR, DECISION_VENDOR, FAVORED_BUYER, FAVORED_VENDOR,
};

use crate::letter::ModeratorRuling;
use crate::moderator::{ModeratorKeys, RulingKey};
use crate::policy::check_payout;
use crate::proofs::tagged;
use crate::terms::EscrowTerms;
use crate::{Error, Party};

impl RulingKey {
    /// The id every filing-fee record names for this escrow.
    pub fn case_id(&self) -> [u8; 32] {
        let txid = self.outpoint.txid.to_byte_array();
        tagged(
            "ob-escrow/case/v1",
            &[&self.terms_digest, &txid, &self.outpoint.vout.to_le_bytes()],
        )
    }
}

/// The digest a fee request cites for the letter it answers.
pub fn fee_letter_digest(letter: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(letter).to_byte_array()
}

/// A dispute decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Ruled(Party),
    Denied(Party),
}

impl Decision {
    fn code(self) -> u8 {
        match self {
            Decision::Ruled(Party::Buyer) => DECISION_BUYER,
            Decision::Ruled(Party::Vendor) => DECISION_VENDOR,
            Decision::Denied(Party::Buyer) => DECISION_DENIED_BUYER,
            Decision::Denied(Party::Vendor) => DECISION_DENIED_VENDOR,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            DECISION_BUYER => Decision::Ruled(Party::Buyer),
            DECISION_VENDOR => Decision::Ruled(Party::Vendor),
            DECISION_DENIED_BUYER => Decision::Denied(Party::Buyer),
            DECISION_DENIED_VENDOR => Decision::Denied(Party::Vendor),
            _ => return None,
        })
    }
}

pub(crate) fn favored_code(party: Party) -> u8 {
    match party {
        Party::Buyer => FAVORED_BUYER,
        Party::Vendor => FAVORED_VENDOR,
    }
}

impl ModeratorKeys {
    fn record_secret(&self) -> Result<SecretKey, Error> {
        SecretKey::from_slice(&self.signing().serialize()).map_err(|_| Error::WrongKey)
    }

    /// The x-only ruling key, as the filing-fee records name it.
    pub fn record_key(&self) -> Result<[u8; 32], Error> {
        let secret = self.record_secret()?;
        Ok(Keypair::from_secret_key(&Secp256k1::new(), &secret)
            .x_only_public_key()
            .0
            .serialize())
    }

    fn sign_record<T: Terms>(&self, terms: T) -> Result<Vec<u8>, Error> {
        let signed = Signed::sign(terms, &self.record_secret()?).map_err(|_| Error::BadPayload)?;
        signed.encode().map_err(|_| Error::BadPayload)
    }

    /// Sign a request. Its `moderator_key` must be [`Self::record_key`].
    pub fn sign_fee_request(&self, request: FeeRequest) -> Result<Vec<u8>, Error> {
        self.sign_record(request)
    }

    pub fn sign_fee_receipt(&self, receipt: FeeReceipt) -> Result<Vec<u8>, Error> {
        self.sign_record(receipt)
    }

    pub fn sign_case_accepted(
        &self,
        case_id: [u8; 32],
        accepted_height: u32,
        due_height: u32,
    ) -> Result<Vec<u8>, Error> {
        self.sign_record(CaseAccepted {
            moderator_key: self.record_key()?,
            case_id,
            accepted_height,
            due_height,
        })
    }

    /// Sign a decision. A ruling must be passed exactly when `decision` rules.
    pub fn sign_dispute_decision(
        &self,
        case_id: [u8; 32],
        decision: Decision,
        ruling_bytes: Option<&[u8]>,
    ) -> Result<Vec<u8>, Error> {
        let ruling_digest = match (decision, ruling_bytes) {
            (Decision::Ruled(party), Some(bytes)) => {
                let ruling = ModeratorRuling::from_bytes(bytes)?;
                let key = RulingKey {
                    terms_digest: ruling.terms_digest(),
                    outpoint: ruling.outpoint(),
                };
                if ruling.favored() != party || key.case_id() != case_id {
                    return Err(Error::BadPayload);
                }
                ruling_digest(bytes)
            }
            (Decision::Denied(_), None) => [0; 32],
            _ => return Err(Error::BadPayload),
        };
        self.sign_record(DisputeDecision {
            moderator_key: self.record_key()?,
            case_id,
            decision: decision.code(),
            ruling_digest,
        })
    }
}

fn xonly(moderator_signing_key: &[u8; 33]) -> [u8; 32] {
    let mut key = [0; 32];
    key.copy_from_slice(&moderator_signing_key[1..]);
    key
}

fn verified<T: Terms>(bytes: &[u8], signer: &[u8; 32]) -> Result<Signed<T>, Error> {
    let signed = Signed::<T>::decode(bytes).map_err(|_| Error::Malformed)?;
    if signed.terms.signer() != *signer {
        return Err(Error::ModeratorMismatch);
    }
    signed.verify().map_err(|_| Error::BadSignature)?;
    Ok(signed)
}

/// What a filer knows about the case it is asked to pay for.
pub struct FeeCase<'a> {
    pub terms: &'a EscrowTerms,
    pub outpoint: OutPoint,
    /// The party that filed, and so the party asked to pay.
    pub filer: Party,
    /// [`fee_letter_digest`] of the letter the filer sent.
    pub letter_digest: [u8; 32],
}

impl FeeCase<'_> {
    pub fn case_id(&self) -> [u8; 32] {
        RulingKey {
            terms_digest: self.terms.digest(),
            outpoint: self.outpoint,
        }
        .case_id()
    }
}

/// Check a request against the filer's own terms. Returns the request and
/// its digest, which the receipt must cite.
///
/// The signer must be the terms' moderator, the amount the terms' non-zero
/// filing fee, the filer the payer, and the script a standard output for
/// that amount. Nothing in the request can change who is paid how much
/// beyond what both parties signed.
pub fn read_fee_request(bytes: &[u8], case: &FeeCase<'_>) -> Result<(FeeRequest, [u8; 32]), Error> {
    let fee = case.terms.moderator.filing_fee().ok_or(Error::BadPayload)?;
    if fee == Amount::ZERO {
        return Err(Error::BadPayload);
    }
    let signed: SignedFeeRequest = verified(bytes, &xonly(&case.terms.moderator.signing_key))?;
    let request = &signed.terms;
    if request.case_id != case.case_id()
        || request.letter_digest != case.letter_digest
        || request.favored != favored_code(case.filer)
        || request.amount_sats != fee.to_sat()
    {
        return Err(Error::BadPayload);
    }
    check_payout(&ScriptBuf::from_bytes(request.script.clone()), fee)?;
    let digest = signed.hash().map_err(|_| Error::Malformed)?;
    Ok((signed.terms, digest))
}

/// Check a receipt for the request with `request_digest`.
pub fn read_fee_receipt(bytes: &[u8], case: &FeeCase<'_>, request_digest: &[u8; 32]) -> Result<FeeReceipt, Error> {
    let signed: SignedFeeReceipt = verified(bytes, &xonly(&case.terms.moderator.signing_key))?;
    if signed.terms.case_id != case.case_id() || signed.terms.request_digest != *request_digest {
        return Err(Error::BadPayload);
    }
    Ok(signed.terms)
}

/// Check a public case-accepted record from the moderator with this key.
pub fn read_case_accepted(bytes: &[u8], moderator_signing_key: &[u8; 33]) -> Result<CaseAccepted, Error> {
    let signed: SignedCaseAccepted = verified(bytes, &xonly(moderator_signing_key))?;
    Ok(signed.terms)
}

/// Check a public decision from the moderator with this key.
pub fn read_dispute_decision(
    bytes: &[u8],
    moderator_signing_key: &[u8; 33],
) -> Result<(Decision, DisputeDecision), Error> {
    let signed: SignedDisputeDecision = verified(bytes, &xonly(moderator_signing_key))?;
    let decision = Decision::from_code(signed.terms.decision).ok_or(Error::BadPayload)?;
    Ok((decision, signed.terms))
}

/// A signed denial of a party's letter and a signed ruling for that same
/// party in the same case: transferable proof the moderator went back on a
/// final denial.
pub fn is_denial_reversed(decision_bytes: &[u8], ruling_bytes: &[u8], moderator_signing_key: &[u8; 33]) -> bool {
    let Ok((Decision::Denied(denied), decision)) = read_dispute_decision(decision_bytes, moderator_signing_key) else {
        return false;
    };
    let Ok(ruling) = ModeratorRuling::from_bytes(ruling_bytes) else {
        return false;
    };
    if ruling.verify(moderator_signing_key).is_err() {
        return false;
    }
    let key = RulingKey {
        terms_digest: ruling.terms_digest(),
        outpoint: ruling.outpoint(),
    };
    key.case_id() == decision.case_id && ruling.favored() == denied
}
