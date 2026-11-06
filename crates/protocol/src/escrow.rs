//! Scriptless escrow summary carried in a dispute envelope.
//!
//! This is not a Taproot script. A visible script leaf is not part of the
//! protocol and is not encoded here.
//!
//! - Happy path: MuSig2 of the buyer and vendor per-order keys.
//! - Vendor claim: presigned, pays the vendor at `vendor_claim_height`.
//! - Buyer refund: presigned, pays the buyer at `refund_locktime`, which is
//!   later than the vendor claim.
//! - Dispute: two adaptor-signed outcomes, one per branch, each paying the
//!   winner plus `moderator_fee`. The moderator can reveal the secret for at
//!   most one branch it was asked about; it never signs a transaction.
//!
//! The claimed branch carries the escrow crate's self-verifying dispute
//! letter as an opaque blob. The other branch's letter is empty unless the
//! counterparty answered with its own.

use crate::encode::{Reader, Writer};
use crate::sign::PUBKEY_LEN;
use crate::Error;

/// Compressed secp256k1 point.
pub const POINT_LEN: usize = 33;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowBranch {
    pub adaptor_point: [u8; POINT_LEN],
    pub letter: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowTerms {
    pub buyer: [u8; PUBKEY_LEN],
    pub vendor: [u8; PUBKEY_LEN],
    /// The moderator's published ruling-signing key (not its encryption key).
    pub moderator: [u8; POINT_LEN],
    /// The fee in the moderator's published record, in sats.
    pub moderator_fee: u64,
    pub dispute_deadline: u32,
    pub vendor_claim_height: u32,
    pub refund_locktime: u32,
    pub buyer_wins: EscrowBranch,
    pub vendor_wins: EscrowBranch,
}

impl EscrowBranch {
    fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.adaptor_point);
        writer.blob(&self.letter)
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            adaptor_point: reader.array()?,
            letter: reader.blob()?,
        })
    }
}

impl EscrowTerms {
    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.buyer);
        writer.bytes(&self.vendor);
        writer.bytes(&self.moderator);
        writer.u64(self.moderator_fee);
        writer.u32(self.dispute_deadline);
        writer.u32(self.vendor_claim_height);
        writer.u32(self.refund_locktime);
        self.buyer_wins.encode(writer)?;
        self.vendor_wins.encode(writer)
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            buyer: reader.array()?,
            vendor: reader.array()?,
            moderator: reader.array()?,
            moderator_fee: reader.u64()?,
            dispute_deadline: reader.u32()?,
            vendor_claim_height: reader.u32()?,
            refund_locktime: reader.u32()?,
            buyer_wins: EscrowBranch::decode(reader)?,
            vendor_wins: EscrowBranch::decode(reader)?,
        })
    }
}
