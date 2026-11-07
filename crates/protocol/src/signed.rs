//! One shape for the small signed records around a listing: coupons, offers,
//! purchase receipts and reviews.
//!
//! The body is `magic || version || fields`, the signature is BIP 340 over
//! the tagged hash of the body, and the encoding is the body followed by the
//! signature. Field checks run on sign and on decode; free-text checks run
//! only on sign, as for listings.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

pub trait Terms: Sized {
    const TAG: &'static str;
    const MAGIC: [u8; 4];
    const VERSION: u8;

    /// The key that must sign these terms.
    fn signer(&self) -> [u8; PUBKEY_LEN];
    fn check(&self) -> Result<(), Error>;
    fn check_public_text(&self) -> Result<(), Error> {
        Ok(())
    }
    fn write(&self, writer: &mut Writer) -> Result<(), Error>;
    fn read(reader: &mut Reader<'_>) -> Result<Self, Error>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed<T> {
    pub terms: T,
    pub signature: [u8; SIG_LEN],
}

impl<T: Terms> Signed<T> {
    pub fn sign(terms: T, secret: &SecretKey) -> Result<Self, Error> {
        if sign::xonly(secret) != terms.signer() {
            return Err(Error::BadKey);
        }
        terms.check_public_text()?;
        let signature = sign::sign(T::TAG, &body(&terms)?, secret);
        Ok(Self { terms, signature })
    }

    pub fn verify(&self) -> Result<(), Error> {
        sign::verify(T::TAG, &body(&self.terms)?, &self.terms.signer(), &self.signature)
    }

    /// SHA-256 of the body, without the signature.
    pub fn hash(&self) -> Result<[u8; 32], Error> {
        Ok(sign::content_hash(&body(&self.terms)?))
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = body(&self.terms)?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        let signed = Self::read_from(&mut reader)?;
        reader.finish()?;
        Ok(signed)
    }

    pub(crate) fn read_from(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.expect_magic(&T::MAGIC, T::VERSION)?;
        let terms = T::read(reader)?;
        terms.check()?;
        Ok(Self { terms, signature: reader.array()? })
    }
}

fn body<T: Terms>(terms: &T) -> Result<Vec<u8>, Error> {
    terms.check()?;
    let mut writer = Writer::new();
    writer.bytes(&T::MAGIC);
    writer.u8(T::VERSION);
    terms.write(&mut writer)?;
    Ok(writer.finish())
}

/// Three uppercase ASCII letters, as in [`crate::FiatPrice`].
pub(crate) fn check_currency(currency: &str) -> Result<(), Error> {
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Error::BadTerms("a currency is three uppercase letters"));
    }
    Ok(())
}
