//! Sent only when someone opens a dispute.
//!
//! The moderator learns the order from this envelope. Nothing here publishes
//! a Taproot script leaf. The sealed variant is an opaque blob.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::escrow::EscrowTerms;
use crate::order::OrderPlaintext;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/dispute/v1";
const MAGIC: [u8; 4] = *b"OBDP";
const VERSION: u8 = 1;
const PLAINTEXT: u8 = 1;
const SEALED: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisputePlaintext {
    pub opener: [u8; PUBKEY_LEN],
    pub order: OrderPlaintext,
    pub claim: String,
    pub escrow: EscrowTerms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisputeEnvelope {
    Plaintext {
        dispute: DisputePlaintext,
        signature: [u8; SIG_LEN],
    },
    Sealed(Vec<u8>),
}

impl DisputePlaintext {
    fn encode_body(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.opener);
        self.order.encode_body(writer)?;
        writer.str(&self.claim)?;
        self.escrow.encode(writer)?;
        Ok(())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.expect_magic(&MAGIC, VERSION)?;
        Ok(Self {
            opener: reader.array()?,
            order: OrderPlaintext::decode_body(reader)?,
            claim: reader.str()?,
            escrow: EscrowTerms::decode(reader)?,
        })
    }

    fn standalone_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        self.encode_body(&mut writer)?;
        Ok(writer.finish())
    }
}

impl DisputeEnvelope {
    pub fn sign(dispute: DisputePlaintext, secret: &SecretKey) -> Result<Self, Error> {
        if sign::xonly(secret) != dispute.opener {
            return Err(Error::BadKey);
        }
        let body = dispute.standalone_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self::Plaintext { dispute, signature })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        match self {
            Self::Plaintext { dispute, signature } => {
                writer.u8(PLAINTEXT);
                dispute.encode_body(&mut writer)?;
                writer.bytes(signature);
            }
            Self::Sealed(bytes) => {
                writer.u8(SEALED);
                writer.blob(bytes)?;
            }
        }
        Ok(writer.finish())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        let envelope = match reader.u8()? {
            PLAINTEXT => {
                let dispute = DisputePlaintext::decode_body(&mut reader)?;
                let signature = reader.array()?;
                Self::Plaintext { dispute, signature }
            }
            SEALED => Self::Sealed(reader.blob()?),
            _ => return Err(Error::UnknownEnvelope),
        };
        reader.finish()?;
        Ok(envelope)
    }

    pub fn verify(&self) -> Result<(), Error> {
        match self {
            Self::Plaintext { dispute, signature } => {
                let body = dispute.standalone_body()?;
                sign::verify(TAG, &body, &dispute.opener, signature)
            }
            Self::Sealed(_) => Err(Error::Sealed),
        }
    }
}
