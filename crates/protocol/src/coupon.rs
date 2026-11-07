//! A promo code signed by the store key.
//!
//! A public coupon is published with the store's catalog; a private one is
//! handed out as its encoded record. Either way the buyer's client checks the
//! store signature, the dates and the scope before ordering. How many times
//! a coupon has been used is known only to the seller's node, which enforces
//! `max_uses` when it prices an order.

use crate::encode::{Reader, Writer};
use crate::listing_details::ListingId;
use crate::signed::{check_currency, Signed, Terms};
use crate::sign::PUBKEY_LEN;
use crate::store::StoreId;
use crate::Error;

pub const MAX_COUPON_LISTINGS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discount {
    /// Basis points off the subtotal it applies to.
    Percent(u16),
    /// A fixed amount off, in minor units, never more than the subtotal.
    Fixed(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coupon {
    pub store_id: StoreId,
    /// Uppercase letters, digits and dashes, 3 to 24 long.
    pub code: String,
    pub discount: Discount,
    pub currency: String,
    /// The subtotal of the lines it applies to must reach this.
    pub min_subtotal: u64,
    /// Sorted, unique. Empty means every listing in the store.
    pub listings: Vec<ListingId>,
    pub starts_on: u32,
    pub ends_on: u32,
    /// Zero means unlimited. Enforced by the seller's node only.
    pub max_uses: u32,
}

pub type SignedCoupon = Signed<Coupon>;

impl Coupon {
    pub fn applies_to(&self, listing: &ListingId) -> bool {
        self.listings.is_empty() || self.listings.binary_search(listing).is_ok()
    }

    pub fn valid_on(&self, day: u32) -> bool {
        (self.starts_on..=self.ends_on).contains(&day)
    }

    /// The canonical form a buyer's typed code is compared in.
    pub fn normalize(code: &str) -> String {
        code.trim().to_ascii_uppercase()
    }
}

impl Terms for Coupon {
    const TAG: &'static str = "ob/coupon/v1";
    const MAGIC: [u8; 4] = *b"OBCP";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        *self.store_id.as_bytes()
    }

    fn check(&self) -> Result<(), Error> {
        let code_ok = (3..=24).contains(&self.code.len())
            && self.code.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-');
        if !code_ok {
            return Err(Error::BadTerms("a coupon code is 3 to 24 uppercase letters, digits or dashes"));
        }
        match self.discount {
            Discount::Percent(bps) if bps == 0 || bps > 10_000 => {
                return Err(Error::BadTerms("a coupon takes 0.01% to 100% off"))
            }
            Discount::Fixed(0) => return Err(Error::BadTerms("a coupon takes more than zero off")),
            _ => {}
        }
        check_currency(&self.currency)?;
        if self.starts_on > self.ends_on {
            return Err(Error::BadTerms("a coupon ends before it starts"));
        }
        if self.listings.len() > MAX_COUPON_LISTINGS || !self.listings.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err(Error::BadTerms("a coupon names at most 32 sorted listings"));
        }
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(self.store_id.as_bytes());
        writer.str(&self.code)?;
        match self.discount {
            Discount::Percent(bps) => {
                writer.u8(0);
                writer.u64(u64::from(bps));
            }
            Discount::Fixed(amount) => {
                writer.u8(1);
                writer.u64(amount);
            }
        }
        writer.str(&self.currency)?;
        writer.u64(self.min_subtotal);
        writer.u8(self.listings.len() as u8);
        for listing in &self.listings {
            writer.bytes(&listing.0);
        }
        writer.u32(self.starts_on);
        writer.u32(self.ends_on);
        writer.u32(self.max_uses);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let store_id = StoreId::from_bytes(reader.array()?);
        let code = reader.str()?;
        let discount = match (reader.u8()?, reader.u64()?) {
            (0, bps) => Discount::Percent(u16::try_from(bps).map_err(|_| Error::BadTerms("a coupon takes 0.01% to 100% off"))?),
            (1, amount) => Discount::Fixed(amount),
            _ => return Err(Error::BadTerms("unknown discount kind")),
        };
        let currency = reader.str()?;
        let min_subtotal = reader.u64()?;
        let count = reader.u8()? as usize;
        if count > MAX_COUPON_LISTINGS {
            return Err(Error::BadTerms("a coupon names at most 32 sorted listings"));
        }
        let listings = (0..count).map(|_| Ok(ListingId(reader.array()?))).collect::<Result<_, Error>>()?;
        Ok(Self {
            store_id,
            code,
            discount,
            currency,
            min_subtotal,
            listings,
            starts_on: reader.u32()?,
            ends_on: reader.u32()?,
            max_uses: reader.u32()?,
        })
    }
}
