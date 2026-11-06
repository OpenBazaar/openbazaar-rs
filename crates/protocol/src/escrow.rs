//! Scriptless escrow terms.
//!
//! This is not a Taproot script. A visible script leaf is not part of the
//! protocol and is not encoded here.
//!
//! - Happy path: MuSig2 of the buyer and vendor spend keys. Partial signatures
//!   are not built in this phase.
//! - Timeout refund: a presigned transaction that carries `refund_locktime`.
//!   The transaction bytes are opaque. This phase does not build or sign one.
//! - Dispute spend: an adaptor signature, so the on-chain payment looks
//!   ordinary. The adaptor signature is not constructed here. The moderator
//!   learns the order only from a dispute envelope.

use crate::encode::{Reader, Writer};
use crate::sign::PUBKEY_LEN;
use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowTerms {
    pub buyer: [u8; PUBKEY_LEN],
    pub vendor: [u8; PUBKEY_LEN],
    pub refund_locktime: u32,
    pub presigned_refund: Vec<u8>,
    pub adaptor_point: [u8; PUBKEY_LEN],
}

impl EscrowTerms {
    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.buyer);
        writer.bytes(&self.vendor);
        writer.u32(self.refund_locktime);
        writer.blob(&self.presigned_refund)?;
        writer.bytes(&self.adaptor_point);
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            buyer: reader.array()?,
            vendor: reader.array()?,
            refund_locktime: reader.u32()?,
            presigned_refund: reader.blob()?,
            adaptor_point: reader.array()?,
        })
    }
}
