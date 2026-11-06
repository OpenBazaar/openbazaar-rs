//! A private letter to one vendor.
//!
//! An order is not a catalog record and not a public relay event. The sealed
//! variant is an opaque blob. This phase does not encrypt or open it.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::listing::ListingHash;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/order/v1";
const MAGIC: [u8; 4] = *b"OBOR";
const VERSION: u8 = 1;
const PLAINTEXT: u8 = 1;
const SEALED: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderPlaintext {
    pub buyer: [u8; PUBKEY_LEN],
    pub listing_hash: ListingHash,
    pub quantity: u32,
    pub note: String,
    pub ship_to: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderEnvelope {
    Plaintext {
        order: OrderPlaintext,
        signature: [u8; SIG_LEN],
    },
    Sealed(Vec<u8>),
}

impl OrderPlaintext {
    pub(crate) fn encode_body(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.buyer);
        writer.bytes(self.listing_hash.as_bytes());
        writer.u32(self.quantity);
        writer.str(&self.note)?;
        writer.str(&self.ship_to)?;
        Ok(())
    }

    pub(crate) fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.expect_magic(&MAGIC, VERSION)?;
        Ok(Self {
            buyer: reader.array()?,
            listing_hash: ListingHash::from_bytes(reader.array()?),
            quantity: reader.u32()?,
            note: reader.str()?,
            ship_to: reader.str()?,
        })
    }

    fn standalone_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        self.encode_body(&mut writer)?;
        Ok(writer.finish())
    }
}

impl OrderEnvelope {
    pub fn sign(order: OrderPlaintext, secret: &SecretKey) -> Result<Self, Error> {
        if sign::xonly(secret) != order.buyer {
            return Err(Error::BadKey);
        }
        let body = order.standalone_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self::Plaintext { order, signature })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        match self {
            Self::Plaintext { order, signature } => {
                writer.u8(PLAINTEXT);
                order.encode_body(&mut writer)?;
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
                let order = OrderPlaintext::decode_body(&mut reader)?;
                let signature = reader.array()?;
                Self::Plaintext { order, signature }
            }
            SEALED => Self::Sealed(reader.blob()?),
            _ => return Err(Error::UnknownEnvelope),
        };
        reader.finish()?;
        Ok(envelope)
    }

    pub fn verify(&self) -> Result<(), Error> {
        match self {
            Self::Plaintext { order, signature } => {
                let body = order.standalone_body()?;
                sign::verify(TAG, &body, &order.buyer, signature)
            }
            Self::Sealed(_) => Err(Error::Sealed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> OrderEnvelope {
        let secret = sign::secret_from_label("order-test");
        OrderEnvelope::sign(
            OrderPlaintext {
                buyer: sign::xonly(&secret),
                listing_hash: ListingHash::from_bytes([7u8; 32]),
                quantity: 1,
                note: "Please pack it flat.".to_string(),
                ship_to: "12 Cedar Street".to_string(),
            },
            &secret,
        )
        .unwrap()
    }

    #[test]
    fn plaintext_round_trip_and_tamper() {
        let envelope = sample();
        let bytes = envelope.encode().unwrap();
        let decoded = OrderEnvelope::decode(&bytes).unwrap();
        assert_eq!(decoded, envelope);
        decoded.verify().unwrap();

        let OrderEnvelope::Plaintext {
            mut order,
            signature,
        } = decoded
        else {
            panic!("plaintext");
        };
        order.ship_to.push('.');
        let tampered = OrderEnvelope::Plaintext { order, signature };
        assert_eq!(tampered.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn sealed_blob_round_trips_without_opening() {
        let envelope = OrderEnvelope::Sealed(b"opaque".to_vec());
        let decoded = OrderEnvelope::decode(&envelope.encode().unwrap()).unwrap();
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.verify(), Err(Error::Sealed));
    }
}
