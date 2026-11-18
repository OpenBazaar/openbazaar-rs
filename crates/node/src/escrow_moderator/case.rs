//! One letter on the moderator's desk, as stored.

use bitcoin::secp256k1::{schnorr, Secp256k1, XOnlyPublicKey};
use bitcoin::OutPoint;
use openbazaar_escrow::Party;
use serde_json::{json, Value};
use tormail::StoreKey;

use crate::escrow_mail::{hex, id_hex, key_bytes, unhex, EscrowId};
use crate::escrow_vendor::offer_digest_with_listing;

#[derive(Clone, Copy)]
pub(super) struct Citation {
    pub listing_id: [u8; 16],
    pub store_key: [u8; 32],
    pub store_signature: [u8; 64],
}

/// The filing-fee terms of a v5 case, fixed by the terms both parties signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CaseFee {
    pub case_id: [u8; 32],
    /// The filing fee in sats. Zero: the moderator takes the case for free.
    pub amount: u64,
    /// `vendor_claim_height`: a paid case should be decided by then.
    pub due: u32,
}

pub(super) struct Case {
    pub letter: Vec<u8>,
    pub memo: String,
    pub outpoint: OutPoint,
    pub received: u32,
    pub reply_mail: StoreKey,
    pub reply_url: String,
    pub ruling: Option<Vec<u8>>,
    pub citation: Option<Citation>,
    pub public_ruling: Option<Vec<u8>>,
    /// The signed outcome beside the ruling: which party was paid
    /// ([`openbazaar_escrow::read_dispute_outcome`]). No store, no listing.
    pub outcome: Option<Vec<u8>>,
    /// The record of this node's own identity the letter was reviewed
    /// under. `None`: the built-in test moderator's.
    pub record: Option<Vec<u8>>,
    /// This node's own shipment lookup for the dispute
    /// ([`crate::escrow_tracking`]). Coarse fields only.
    pub tracking: Option<Value>,
    /// `None` for a v4 case, which has no filing fee.
    pub fee: Option<CaseFee>,
    /// The signed decision on this letter (`OBDD`): a ruling or a denial.
    pub decision: Option<Vec<u8>>,
}

fn optional_hex(value: &Value) -> Option<Option<Vec<u8>>> {
    match value.as_str() {
        Some(text) => Some(Some(unhex(text).ok()?)),
        None => Some(None),
    }
}

impl Case {
    pub fn to_record(&self) -> Value {
        json!({
            "letter": hex(&self.letter),
            "memo": self.memo,
            "outpoint": self.outpoint.to_string(),
            "received": self.received,
            "replyMail": hex(self.reply_mail.as_bytes()),
            "replyUrl": self.reply_url,
            "ruling": self.ruling.as_deref().map(hex),
            "listingId": self.citation.map(|citation| hex(&citation.listing_id)),
            "storeKey": self.citation.map(|citation| hex(&citation.store_key)),
            "storeSignature": self.citation.map(|citation| hex(&citation.store_signature)),
            "publicRuling": self.public_ruling.as_deref().map(hex),
            "outcome": self.outcome.as_deref().map(hex),
            "record": self.record.as_deref().map(hex),
            "tracking": self.tracking.clone(),
            "fee": self.fee.map(|fee| json!({ "caseId": hex(&fee.case_id), "amountSats": fee.amount, "dueHeight": fee.due })),
            "decision": self.decision.as_deref().map(hex),
        })
    }

    pub fn from_record(value: &Value) -> Option<Self> {
        let fee = match &value["fee"] {
            Value::Null => None,
            fee => Some(CaseFee {
                case_id: fixed(fee["caseId"].as_str()?)?,
                amount: fee["amountSats"].as_u64()?,
                due: u32::try_from(fee["dueHeight"].as_u64()?).ok()?,
            }),
        };
        Some(Self {
            letter: unhex(value["letter"].as_str()?).ok()?,
            memo: value["memo"].as_str()?.to_string(),
            outpoint: value["outpoint"].as_str()?.parse().ok()?,
            received: u32::try_from(value["received"].as_u64()?).ok()?,
            reply_mail: key_bytes(&unhex(value["replyMail"].as_str()?).ok()?).ok()?,
            reply_url: value["replyUrl"].as_str()?.to_string(),
            ruling: optional_hex(&value["ruling"])?,
            citation: citation_from(value),
            public_ruling: optional_hex(&value["publicRuling"])?,
            outcome: optional_hex(&value["outcome"])?,
            record: optional_hex(&value["record"])?,
            tracking: match &value["tracking"] {
                Value::Null => None,
                found => Some(found.clone()),
            },
            fee,
            decision: optional_hex(&value["decision"])?,
        })
    }

    /// No payment is owed before the operator may decide: a v4 case or a free one.
    pub fn free(&self) -> bool {
        self.fee.is_none_or(|fee| fee.amount == 0)
    }
}

pub(super) fn branch_byte(name: &str) -> Option<u8> {
    match name {
        "buyer" => Some(Party::Buyer as u8),
        "vendor" => Some(Party::Vendor as u8),
        _ => None,
    }
}

pub(super) fn party_of(byte: u8) -> Party {
    if byte == Party::Vendor as u8 {
        Party::Vendor
    } else {
        Party::Buyer
    }
}

pub(super) fn party_name(party: Party) -> &'static str {
    match party {
        Party::Buyer => "buyer",
        Party::Vendor => "vendor",
    }
}

pub(super) fn case_name(id: &EscrowId, party: Party) -> String {
    format!("{}-{}", id_hex(id), party_name(party))
}

pub(super) fn fixed<const N: usize>(text: &str) -> Option<[u8; N]> {
    unhex(text).ok().and_then(|bytes| bytes.as_slice().try_into().ok())
}

fn citation_from(value: &Value) -> Option<Citation> {
    Some(Citation {
        listing_id: fixed(value["listingId"].as_str()?)?,
        store_key: fixed(value["storeKey"].as_str()?)?,
        store_signature: fixed(value["storeSignature"].as_str()?)?,
    })
}

pub(super) fn citation_from_fields(fields: &[Vec<u8>]) -> Result<Citation, String> {
    Ok(Citation {
        listing_id: fields[3].as_slice().try_into().map_err(|_| "bad listing id")?,
        store_key: fields[4].as_slice().try_into().map_err(|_| "bad store key")?,
        store_signature: fields[5].as_slice().try_into().map_err(|_| "bad store signature")?,
    })
}

pub(super) fn check_citation(
    id: &EscrowId,
    terms: &openbazaar_escrow::EscrowTerms,
    citation: &Citation,
) -> Result<(), String> {
    let key = XOnlyPublicKey::from_slice(&citation.store_key).map_err(|err| err.to_string())?;
    let signature = schnorr::Signature::from_slice(&citation.store_signature).map_err(|err| err.to_string())?;
    Secp256k1::new()
        .verify_schnorr(
            &signature,
            &offer_digest_with_listing(id, terms, &citation.listing_id),
            &key,
        )
        .map_err(|_| "the store signature does not cover this listing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_stored_before_moderators_had_their_own_keys_is_the_built_in_moderators() {
        let mut case = Case {
            letter: vec![1, 2, 3],
            memo: "Never arrived.".into(),
            outpoint: OutPoint::null(),
            received: 120,
            reply_mail: key_bytes(&[7; 32]).unwrap(),
            reply_url: "http://127.0.0.1:9000".into(),
            ruling: None,
            citation: None,
            public_ruling: None,
            outcome: None,
            record: Some(vec![9; 40]),
            tracking: Some(json!({ "status": "delivered" })),
            fee: Some(CaseFee {
                case_id: [4; 32],
                amount: 5_000,
                due: 300,
            }),
            decision: Some(vec![5; 10]),
        };
        let own = Case::from_record(&case.to_record()).unwrap();
        assert_eq!(
            own.record,
            Some(vec![9; 40]),
            "a case remembers the record it was reviewed under"
        );
        assert_eq!(
            own.tracking,
            Some(json!({ "status": "delivered" })),
            "a case remembers its shipment lookup"
        );
        assert_eq!(own.fee, case.fee, "a v5 case remembers its filing fee");
        assert_eq!(own.decision, Some(vec![5; 10]));
        assert!(!own.free());
        case.record = None;
        case.tracking = None;
        case.outcome = None;
        let mut old = case.to_record();
        for key in ["record", "tracking", "outcome", "fee", "decision"] {
            old.as_object_mut().unwrap().remove(key);
        }
        let old = Case::from_record(&old).unwrap();
        assert!(old.free());
        assert_eq!(
            (old.record, old.memo.as_str(), old.received),
            (None, "Never arrived.", 120)
        );
        assert_eq!(
            old.tracking, None,
            "a case stored before dispute lookups existed has none"
        );
        assert_eq!(
            old.outcome, None,
            "a case stored before signed outcomes existed has none"
        );
        assert_eq!(
            (old.fee, old.decision),
            (None, None),
            "a case stored before filing fees is a v4 case"
        );
    }
}
