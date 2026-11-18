//! The records around a dispute filing fee, all signed by the moderator's
//! ruling key and bound to one case by its case id.
//!
//! A [`FeeRequest`] and a [`FeeReceipt`] go only to the filer. A
//! [`CaseAccepted`] and a [`DisputeDecision`] are public: they name the case
//! id, never a payment, a party, or a listing, so anyone can see that a paid
//! case was decided, or that it is overdue.

use crate::encode::{Reader, Writer};
use crate::reputation::refuse_private;
use crate::sign::PUBKEY_LEN;
use crate::signed::{Signed, Terms};
use crate::{Error, FAVORED_BUYER, FAVORED_VENDOR};

const MAX_BOLT11: usize = 2048;
const MAX_NETWORK: usize = 16;
/// A P2TR script is 34 bytes; anything a fee request asks for fits here.
const MAX_FEE_SCRIPT: usize = 64;

/// How a filer can pay a request over Lightning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeeInvoice {
    /// The moderator's Lightning network, as its node names it (`signet`, `bitcoin`).
    pub network: String,
    pub bolt11: String,
    pub payment_hash: [u8; 32],
}

/// The moderator will take the case once `amount_sats` reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeeRequest {
    pub moderator_key: [u8; PUBKEY_LEN],
    pub case_id: [u8; 32],
    /// Hash of the dispute letter this request answers.
    pub letter_digest: [u8; 32],
    /// [`FAVORED_BUYER`] or [`FAVORED_VENDOR`]: the filer.
    pub favored: u8,
    /// The filing fee in the terms both parties signed.
    pub amount_sats: u64,
    pub invoice: Option<FeeInvoice>,
    /// A fresh output script of the moderator's, used for no other request.
    pub script: Vec<u8>,
    /// Last height at which the moderator still takes payment for this request.
    pub valid_until_height: u32,
    /// The height by which a paid case should be decided.
    pub due_height: u32,
}

pub type SignedFeeRequest = Signed<FeeRequest>;

/// How a request was paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeePayment {
    Lightning { payment_hash: [u8; 32] },
    OnChain { txid: [u8; 32], vout: u32 },
}

const RAIL_LIGHTNING: u8 = 1;
const RAIL_ON_CHAIN: u8 = 2;

/// The moderator received the fee for a case. Private to the filer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeeReceipt {
    pub moderator_key: [u8; PUBKEY_LEN],
    pub case_id: [u8; 32],
    /// [`Signed::hash`] of the request that was paid.
    pub request_digest: [u8; 32],
    pub payment: FeePayment,
    pub received_sats: u64,
    pub paid_height: u32,
    pub due_height: u32,
}

pub type SignedFeeReceipt = Signed<FeeReceipt>;

/// Public: the moderator was paid for a case and owes a decision by `due_height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseAccepted {
    pub moderator_key: [u8; PUBKEY_LEN],
    pub case_id: [u8; 32],
    pub accepted_height: u32,
    pub due_height: u32,
}

pub type SignedCaseAccepted = Signed<CaseAccepted>;

/// `decision` in a [`DisputeDecision`]: ruled for the buyer.
pub const DECISION_BUYER: u8 = FAVORED_BUYER;
/// Ruled for the vendor.
pub const DECISION_VENDOR: u8 = FAVORED_VENDOR;
/// Refused the buyer's letter. Final; the fee is not returned.
pub const DECISION_DENIED_BUYER: u8 = 3;
/// Refused the vendor's letter.
pub const DECISION_DENIED_VENDOR: u8 = 4;

/// Public: what the moderator decided for one letter of a case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisputeDecision {
    pub moderator_key: [u8; PUBKEY_LEN],
    pub case_id: [u8; 32],
    pub decision: u8,
    /// [`crate::ruling_digest`] of the ruling for a ruled decision; zero for a denial.
    pub ruling_digest: [u8; 32],
}

pub type SignedDisputeDecision = Signed<DisputeDecision>;

impl DisputeDecision {
    pub fn is_denial(&self) -> bool {
        matches!(self.decision, DECISION_DENIED_BUYER | DECISION_DENIED_VENDOR)
    }

    /// The party whose letter this decision answers.
    pub fn filer(&self) -> u8 {
        match self.decision {
            DECISION_DENIED_BUYER => FAVORED_BUYER,
            DECISION_DENIED_VENDOR => FAVORED_VENDOR,
            ruled => ruled,
        }
    }
}

fn check_favored(favored: u8) -> Result<(), Error> {
    if favored != FAVORED_BUYER && favored != FAVORED_VENDOR {
        return Err(Error::BadTerms("a dispute fee names the buyer or the vendor"));
    }
    Ok(())
}

fn check_heights(first: u32, due: u32) -> Result<(), Error> {
    if first == 0 || first > due {
        return Err(Error::BadTerms(
            "a dispute fee height is non-zero and not after the due height",
        ));
    }
    Ok(())
}

/// A payment can land after the due height while the case is still open.
fn check_nonzero(at: u32, due: u32) -> Result<(), Error> {
    if at == 0 || due == 0 {
        return Err(Error::BadTerms("a dispute fee height is non-zero"));
    }
    Ok(())
}

impl Terms for FeeRequest {
    const TAG: &'static str = "ob/dispute-fee-request/v1";
    const MAGIC: [u8; 4] = *b"OBFQ";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        check_favored(self.favored)?;
        check_heights(self.valid_until_height, self.due_height)?;
        if self.amount_sats == 0 {
            return Err(Error::BadTerms("a zero filing fee needs no request"));
        }
        if self.script.is_empty() || self.script.len() > MAX_FEE_SCRIPT {
            return Err(Error::BadTerms("a fee request names one output script"));
        }
        if let Some(invoice) = &self.invoice {
            let ascii = |text: &str| text.bytes().all(|b| b.is_ascii_graphic());
            if invoice.network.is_empty()
                || invoice.network.len() > MAX_NETWORK
                || invoice.bolt11.is_empty()
                || invoice.bolt11.len() > MAX_BOLT11
                || !ascii(&invoice.network)
                || !ascii(&invoice.bolt11)
            {
                return Err(Error::BadTerms("a fee invoice is a network name and a BOLT 11 string"));
            }
        }
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.moderator_key);
        writer.bytes(&self.case_id);
        writer.bytes(&self.letter_digest);
        writer.u8(self.favored);
        writer.u64(self.amount_sats);
        match &self.invoice {
            None => writer.u8(0),
            Some(invoice) => {
                writer.u8(1);
                writer.str(&invoice.network)?;
                writer.str(&invoice.bolt11)?;
                writer.bytes(&invoice.payment_hash);
            }
        }
        writer.blob(&self.script)?;
        writer.u32(self.valid_until_height);
        writer.u32(self.due_height);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let moderator_key = reader.array()?;
        let case_id = reader.array()?;
        let letter_digest = reader.array()?;
        let favored = reader.u8()?;
        let amount_sats = reader.u64()?;
        let invoice = match reader.u8()? {
            0 => None,
            1 => Some(FeeInvoice {
                network: reader.str()?,
                bolt11: reader.str()?,
                payment_hash: reader.array()?,
            }),
            _ => return Err(Error::BadTerms("a fee invoice is present or absent")),
        };
        Ok(Self {
            moderator_key,
            case_id,
            letter_digest,
            favored,
            amount_sats,
            invoice,
            script: reader.blob()?,
            valid_until_height: reader.u32()?,
            due_height: reader.u32()?,
        })
    }
}

impl Terms for FeeReceipt {
    const TAG: &'static str = "ob/dispute-fee-receipt/v1";
    const MAGIC: [u8; 4] = *b"OBFR";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        check_nonzero(self.paid_height, self.due_height)?;
        if self.received_sats == 0 {
            return Err(Error::BadTerms("a fee receipt names a payment"));
        }
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.moderator_key);
        writer.bytes(&self.case_id);
        writer.bytes(&self.request_digest);
        match self.payment {
            FeePayment::Lightning { payment_hash } => {
                writer.u8(RAIL_LIGHTNING);
                writer.bytes(&payment_hash);
            }
            FeePayment::OnChain { txid, vout } => {
                writer.u8(RAIL_ON_CHAIN);
                writer.bytes(&txid);
                writer.u32(vout);
            }
        }
        writer.u64(self.received_sats);
        writer.u32(self.paid_height);
        writer.u32(self.due_height);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let moderator_key = reader.array()?;
        let case_id = reader.array()?;
        let request_digest = reader.array()?;
        let payment = match reader.u8()? {
            RAIL_LIGHTNING => FeePayment::Lightning {
                payment_hash: reader.array()?,
            },
            RAIL_ON_CHAIN => FeePayment::OnChain {
                txid: reader.array()?,
                vout: reader.u32()?,
            },
            _ => return Err(Error::BadTerms("a fee is paid over Lightning or on chain")),
        };
        Ok(Self {
            moderator_key,
            case_id,
            request_digest,
            payment,
            received_sats: reader.u64()?,
            paid_height: reader.u32()?,
            due_height: reader.u32()?,
        })
    }
}

impl Terms for CaseAccepted {
    const TAG: &'static str = "ob/case-accepted/v1";
    const MAGIC: [u8; 4] = *b"OBCA";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        check_nonzero(self.accepted_height, self.due_height)?;
        refuse_private(&self.case_id)?;
        refuse_private(&self.moderator_key)
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.moderator_key);
        writer.bytes(&self.case_id);
        writer.u32(self.accepted_height);
        writer.u32(self.due_height);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            moderator_key: reader.array()?,
            case_id: reader.array()?,
            accepted_height: reader.u32()?,
            due_height: reader.u32()?,
        })
    }
}

impl Terms for DisputeDecision {
    const TAG: &'static str = "ob/dispute-decision/v1";
    const MAGIC: [u8; 4] = *b"OBDD";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        let ruled = matches!(self.decision, DECISION_BUYER | DECISION_VENDOR);
        if !ruled && !self.is_denial() {
            return Err(Error::BadTerms("a decision rules for a party or denies a letter"));
        }
        if ruled == (self.ruling_digest == [0; 32]) {
            return Err(Error::BadTerms(
                "a ruled decision cites its ruling; a denial cites none",
            ));
        }
        refuse_private(&self.case_id)?;
        refuse_private(&self.ruling_digest)?;
        refuse_private(&self.moderator_key)
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.moderator_key);
        writer.bytes(&self.case_id);
        writer.u8(self.decision);
        writer.bytes(&self.ruling_digest);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            moderator_key: reader.array()?,
            case_id: reader.array()?,
            decision: reader.u8()?,
            ruling_digest: reader.array()?,
        })
    }
}
