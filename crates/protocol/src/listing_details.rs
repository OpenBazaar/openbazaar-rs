//! What a version 6 listing adds to version 5: a stable id, images, and an
//! optional price in a fiat currency.
//!
//! A listing's address is the hash of its body, so every edit moves it. The
//! id stays the same across edits, so a store, its buyers, and the nodes
//! that replicate it can tell "a new version of that listing" from "a new
//! listing". Images are named by the SHA-256 of their bytes, as in the store
//! profile. A fiat price is what the seller asked for; the signed sats price
//! next to it is that amount at the rate on the day it was signed, and the
//! seller's node converts the fiat price again when it answers an order.

use crate::encode::{Reader, Writer};
use crate::Error;

/// Most images one listing names.
pub const MAX_IMAGES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListingId(pub [u8; 16]);

impl ListingId {
    pub fn to_hex(self) -> String {
        crate::encode::to_hex(&self.0)
    }
}

/// A price set in a fiat currency, in that currency's minor unit (cents for
/// USD, EUR, and GBP; yen for JPY).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiatPrice {
    /// ISO 4217 code: three uppercase letters.
    pub currency: String,
    pub amount: u64,
    /// Flat shipping for one package, in the same unit. Zero for digital goods.
    pub shipping: u64,
}

impl FiatPrice {
    /// Minor units per major unit of `currency`.
    pub fn scale(currency: &str) -> u64 {
        match currency {
            "JPY" => 1,
            _ => 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListingDetails {
    pub id: ListingId,
    /// SHA-256 of each image, in display order. At most [`MAX_IMAGES`], no repeats.
    pub images: Vec<[u8; 32]>,
    pub price: Option<FiatPrice>,
}

impl ListingDetails {
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.images.len() > MAX_IMAGES {
            return Err(Error::BadListing("a listing names at most 8 images"));
        }
        for (i, image) in self.images.iter().enumerate() {
            if self.images[..i].contains(image) {
                return Err(Error::BadListing("an image is named twice"));
            }
        }
        if let Some(price) = &self.price {
            if price.currency.len() != 3 || !price.currency.bytes().all(|b| b.is_ascii_uppercase()) {
                return Err(Error::BadListing("a currency is three uppercase letters"));
            }
            if price.amount == 0 {
                return Err(Error::BadListing("a fiat price is more than zero"));
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        self.check()?;
        writer.bytes(&self.id.0);
        writer.u8(self.images.len() as u8);
        for image in &self.images {
            writer.bytes(image);
        }
        match &self.price {
            None => writer.u8(0),
            Some(price) => {
                writer.u8(1);
                writer.str(&price.currency)?;
                writer.u64(price.amount);
                writer.u64(price.shipping);
            }
        }
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let id = ListingId(reader.array()?);
        let count = reader.u8()? as usize;
        if count > MAX_IMAGES {
            return Err(Error::BadListing("a listing names at most 8 images"));
        }
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            images.push(reader.array()?);
        }
        let price = match reader.u8()? {
            0 => None,
            1 => Some(FiatPrice { currency: reader.str()?, amount: reader.u64()?, shipping: reader.u64()? }),
            _ => return Err(Error::BadListing("unknown price tag")),
        };
        let details = Self { id, images, price };
        details.check()?;
        Ok(details)
    }
}
