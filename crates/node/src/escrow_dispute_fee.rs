//! The filer's side of a v5 filing fee, for the buyer and the vendor alike.
//!
//! A party that sends a dispute letter under v5 terms remembers the
//! letter's digest. The moderator answers a reviewed letter with a signed
//! request ([`Kind::FeeRequest`]); it is kept only if it names the terms'
//! moderator, this escrow, this party, a letter this node sent, and exactly
//! the fee both parties signed ([`openbazaar_escrow::read_fee_request`]).
//! Nothing in a request can raise the fee or redirect it to another case.
//!
//! The party pays from [`pay`] after one passphrase confirmation. The
//! moderator's signed receipt ([`Kind::FeeReceipt`]) or denial
//! ([`Kind::Denied`]) is checked the same way and kept beside the request.
//! Records are `escrow/filer/{id}-{buyer|vendor}`, sealed like every escrow
//! record; none holds a secret.

pub(crate) mod pay;

use bitcoin::{OutPoint, Txid};
use openbazaar_escrow::{
    fee_letter_digest, read_dispute_decision, read_fee_receipt, read_fee_request, Decision, EscrowTerms, FeeCase,
    FeeKind, Party,
};
use openbazaar_protocol::{FeeRequest, SignedFeeRequest};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, id_hex, unhex, EscrowId, Kind, Mail, RETRY_LATER};
use crate::escrow_store::Role;
use crate::Node;

/// Letters and requests kept per escrow; a party that re-sends more often
/// than this keeps the newest.
const MAX_KEPT: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Paid {
    Lightning { payment_hash: String },
    OnChain { txid: Txid, vout: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Payment {
    /// The digest of the request it paid.
    pub request: [u8; 32],
    pub paid: Paid,
    pub amount_sats: u64,
    /// The network or routing fee this node paid on top.
    pub fee_sats: u64,
}

/// One party's filing fee in one escrow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FilerFee {
    pub letters: Vec<[u8; 32]>,
    /// Signed requests, oldest first.
    pub requests: Vec<Vec<u8>>,
    pub payment: Option<Payment>,
    pub receipt: Option<Vec<u8>>,
    pub denied: Option<Vec<u8>>,
}

fn fixed32(text: &str) -> Option<[u8; 32]> {
    unhex(text).ok()?.try_into().ok()
}

fn optional(value: &Value) -> Option<Option<Vec<u8>>> {
    match value.as_str() {
        Some(text) => unhex(text).ok().map(Some),
        None => Some(None),
    }
}

impl FilerFee {
    fn to_record(&self) -> Value {
        json!({
            "letters": self.letters.iter().map(|digest| hex(digest)).collect::<Vec<_>>(),
            "requests": self.requests.iter().map(|bytes| hex(bytes)).collect::<Vec<_>>(),
            "payment": self.payment.as_ref().map(|payment| json!({
                "request": hex(&payment.request),
                "rail": match payment.paid { Paid::Lightning { .. } => "ln", Paid::OnChain { .. } => "onchain" },
                "paymentHash": match &payment.paid { Paid::Lightning { payment_hash } => Some(payment_hash.clone()), Paid::OnChain { .. } => None },
                "txid": match payment.paid { Paid::OnChain { txid, .. } => Some(txid.to_string()), Paid::Lightning { .. } => None },
                "vout": match payment.paid { Paid::OnChain { vout, .. } => Some(vout), Paid::Lightning { .. } => None },
                "amountSats": payment.amount_sats,
                "feeSats": payment.fee_sats,
            })),
            "receipt": self.receipt.as_deref().map(hex),
            "denied": self.denied.as_deref().map(hex),
        })
    }

    fn from_record(value: &Value) -> Option<Self> {
        let payment = match &value["payment"] {
            Value::Null => None,
            payment => Some(Payment {
                request: fixed32(payment["request"].as_str()?)?,
                paid: match payment["rail"].as_str()? {
                    "ln" => Paid::Lightning {
                        payment_hash: payment["paymentHash"].as_str()?.to_string(),
                    },
                    "onchain" => Paid::OnChain {
                        txid: payment["txid"].as_str()?.parse().ok()?,
                        vout: u32::try_from(payment["vout"].as_u64()?).ok()?,
                    },
                    _ => return None,
                },
                amount_sats: payment["amountSats"].as_u64()?,
                fee_sats: payment["feeSats"].as_u64()?,
            }),
        };
        Some(Self {
            letters: value["letters"]
                .as_array()?
                .iter()
                .map(|text| fixed32(text.as_str()?))
                .collect::<Option<_>>()?,
            requests: value["requests"]
                .as_array()?
                .iter()
                .map(|text| unhex(text.as_str()?).ok())
                .collect::<Option<_>>()?,
            payment,
            receipt: optional(&value["receipt"])?,
            denied: optional(&value["denied"])?,
        })
    }

    /// The newest request and its digest.
    pub(crate) fn latest(&self) -> Option<(FeeRequest, [u8; 32])> {
        let signed = SignedFeeRequest::decode(self.requests.last()?).ok()?;
        let digest = signed.hash().ok()?;
        Some((signed.terms, digest))
    }

    fn holds(&self, digest: &[u8; 32]) -> bool {
        self.requests
            .iter()
            .filter_map(|bytes| SignedFeeRequest::decode(bytes).ok()?.hash().ok())
            .any(|held| held == *digest)
    }

    /// Where this fee stands, for the order or sale it belongs to.
    pub(crate) fn state(&self) -> &'static str {
        match (&self.denied, &self.receipt, &self.payment, self.requests.is_empty()) {
            (Some(_), _, _, _) => "denied",
            (None, Some(_), _, _) => "paid",
            (None, None, Some(_), _) => "paying",
            (None, None, None, false) => "requested",
            (None, None, None, true) => "awaiting-request",
        }
    }

    fn value(&self, tip: Option<u32>) -> Value {
        let latest = self.latest();
        json!({
            "state": self.state(),
            "amountSats": latest.as_ref().map(|(request, _)| request.amount_sats),
            "validUntilHeight": latest.as_ref().map(|(request, _)| request.valid_until_height),
            "dueHeight": latest.as_ref().map(|(request, _)| request.due_height),
            "expired": latest.as_ref().zip(tip).is_some_and(|((request, _), tip)| request.valid_until_height < tip),
            "lightning": latest.as_ref().is_some_and(|(request, _)| request.invoice.is_some()),
            "rail": self.payment.as_ref().map(|payment| match payment.paid { Paid::Lightning { .. } => "ln", Paid::OnChain { .. } => "onchain" }),
            "txid": self.payment.as_ref().and_then(|payment| match payment.paid { Paid::OnChain { txid, .. } => Some(txid.to_string()), Paid::Lightning { .. } => None }),
            "receipt": self.receipt.as_deref().map(hex),
            "denial": self.denied.as_deref().map(hex),
        })
    }
}

fn role_name(party: Party) -> &'static str {
    match party {
        Party::Buyer => "buyer",
        Party::Vendor => "vendor",
    }
}

fn record_name(party: Party, id: &EscrowId) -> String {
    format!("{}-{}", id_hex(id), role_name(party))
}

/// What a party knows of its own escrow: the terms it signed and the funding.
pub(crate) struct FiledEscrow {
    pub terms: EscrowTerms,
    pub outpoint: OutPoint,
}

impl FiledEscrow {
    fn fee_case(&self, filer: Party, letter_digest: [u8; 32]) -> FeeCase<'_> {
        FeeCase {
            terms: &self.terms,
            outpoint: self.outpoint,
            filer,
            letter_digest,
        }
    }
}

impl Node {
    pub(crate) fn filer_fee(&self, party: Party, id: &EscrowId) -> Result<FilerFee, String> {
        match self.escrow.store.load(Role::Filer, &record_name(party, id))? {
            Some(record) => {
                FilerFee::from_record(&record).ok_or_else(|| "the filing fee record could not be read".into())
            }
            None => Ok(FilerFee::default()),
        }
    }

    pub(crate) fn save_filer_fee(&self, party: Party, id: &EscrowId, fee: &FilerFee) -> Result<(), String> {
        self.escrow
            .store
            .save(Role::Filer, &record_name(party, id), &fee.to_record())?;
        if party == Party::Buyer {
            self.mirror_buyer_fee(id, self.filer_fee_value(party, id))?;
        }
        Ok(())
    }

    /// Remember a letter this node sent, so a request for it is recognized.
    /// Under v4 terms there is no fee, and nothing is kept.
    pub(crate) fn note_dispute_letter(
        &self,
        party: Party,
        id: &EscrowId,
        terms: &EscrowTerms,
        letter: &[u8],
    ) -> Result<(), String> {
        if terms.moderator.fee_kind != FeeKind::Filing {
            return Ok(());
        }
        let mut fee = self.filer_fee(party, id)?;
        let digest = fee_letter_digest(letter);
        if !fee.letters.contains(&digest) {
            fee.letters.push(digest);
            if fee.letters.len() > MAX_KEPT {
                fee.letters.remove(0);
            }
        }
        self.save_filer_fee(party, id, &fee)
    }

    /// Refuse a new letter from a party whose letter the moderator denied.
    pub(crate) fn refuse_after_denial(&self, party: Party, id: &EscrowId) -> Result<(), String> {
        match self.filer_fee(party, id)?.denied {
            Some(_) => Err("The moderator denied this dispute. A denial is final.".into()),
            None => Ok(()),
        }
    }

    /// The fee fields of an order or sale; `Null` when it has none.
    pub(crate) fn filer_fee_value(&self, party: Party, id: &EscrowId) -> Value {
        match self.filer_fee(party, id) {
            Ok(fee) if fee != FilerFee::default() => fee.value(self.chain.tip_height().ok()),
            _ => Value::Null,
        }
    }

    /// A request, receipt, or denial from the moderator for `party`'s letter.
    pub(crate) fn filer_mail(&self, party: Party, escrow: &FiledEscrow, mail: &Mail) -> Result<(), String> {
        if escrow.terms.moderator.fee_kind != FeeKind::Filing {
            return Err("this escrow has no filing fee".into());
        }
        let bytes = mail.fields(1)?[0].clone();
        let mut fee = self.filer_fee(party, &mail.escrow)?;
        match mail.kind {
            Kind::FeeRequest => {
                let digest = fee
                    .letters
                    .iter()
                    .find_map(|letter| read_fee_request(&bytes, &escrow.fee_case(party, *letter)).ok())
                    .map(|(_, digest)| digest)
                    .ok_or("the fee request does not answer a letter this node sent")?;
                if fee.holds(&digest) {
                    return Ok(());
                }
                fee.requests.push(bytes);
                if fee.requests.len() > MAX_KEPT {
                    let paid = fee.payment.as_ref().map(|payment| payment.request);
                    let drop = fee.requests.iter().position(|held| {
                        SignedFeeRequest::decode(held)
                            .ok()
                            .and_then(|signed| signed.hash().ok())
                            != paid
                    });
                    fee.requests.remove(drop.unwrap_or(0));
                }
            }
            Kind::FeeReceipt => {
                let case = escrow.fee_case(party, [0; 32]);
                let held = fee
                    .requests
                    .iter()
                    .filter_map(|held| SignedFeeRequest::decode(held).ok()?.hash().ok());
                if !held
                    .into_iter()
                    .any(|digest| read_fee_receipt(&bytes, &case, &digest).is_ok())
                {
                    // Mail can overtake the request it answers; only the
                    // moderator's pinned key can send this, so it waits.
                    return Err(format!("{RETRY_LATER}the receipt cites a request not here yet"));
                }
                fee.receipt = Some(bytes);
            }
            Kind::Denied => {
                let (decision, signed) = read_dispute_decision(&bytes, &escrow.terms.moderator.signing_key)
                    .map_err(|err| err.to_string())?;
                if decision != Decision::Denied(party) || signed.case_id != escrow.fee_case(party, [0; 32]).case_id() {
                    return Err("the denial is not for this party's letter in this escrow".into());
                }
                fee.denied = Some(bytes);
            }
            other => return Err(format!("{other:?} is not filing-fee mail")),
        }
        self.save_filer_fee(party, &mail.escrow, &fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filer_fee_survives_its_record() {
        let mut fee = FilerFee {
            letters: vec![[1; 32]],
            requests: vec![vec![2, 3]],
            ..FilerFee::default()
        };
        assert_eq!(fee.state(), "requested");
        fee.payment = Some(Payment {
            request: [4; 32],
            paid: Paid::OnChain {
                txid: Txid::from_raw_hash(bitcoin::hashes::Hash::all_zeros()),
                vout: 1,
            },
            amount_sats: 5_000,
            fee_sats: 300,
        });
        assert_eq!(fee.state(), "paying");
        assert_eq!(FilerFee::from_record(&fee.to_record()), Some(fee.clone()));
        fee.payment.as_mut().unwrap().paid = Paid::Lightning {
            payment_hash: "ab".into(),
        };
        fee.receipt = Some(vec![5]);
        assert_eq!(FilerFee::from_record(&fee.to_record()), Some(fee.clone()));
        assert_eq!(fee.state(), "paid");
        fee.denied = Some(vec![6]);
        assert_eq!(fee.state(), "denied");
        assert_eq!(FilerFee::from_record(&json!({})), None);
    }
}
