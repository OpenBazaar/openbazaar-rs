//! Verified-purchase reviews under per-order pseudonymous keys.
//!
//! The buyer derives a review key for each order and sends its public half
//! in the order letter. Once the order is paid, the seller signs a
//! [`Receipt`]: "this key bought this listing". The buyer signs a [`Review`]
//! with that key and the receipt inside it. Nothing in either names the
//! order, the amount, the buyer's other keys, or a time finer than a day, so
//! two reviews by one buyer do not link and a review does not point at a
//! payment on chain.
//!
//! What this cannot stop: a seller minting receipts for keys it controls.
//! A client shows the count of verified reviews next to the store's age and
//! sales, not as proof of independent buyers.

use crate::encode::{Reader, Writer};
use crate::listing_details::ListingId;
use crate::public_text;
use crate::signed::{Signed, Terms};
use crate::sign::PUBKEY_LEN;
use crate::store::StoreId;
use crate::Error;

pub const MAX_REVIEW_TEXT: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub store_id: StoreId,
    pub listing_id: ListingId,
    pub review_key: [u8; PUBKEY_LEN],
    /// YYYYMMDD.
    pub paid_on: u32,
}

pub type SignedReceipt = Signed<Receipt>;

impl Terms for Receipt {
    const TAG: &'static str = "ob/receipt/v1";
    const MAGIC: [u8; 4] = *b"OBRC";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        *self.store_id.as_bytes()
    }

    fn check(&self) -> Result<(), Error> {
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(self.store_id.as_bytes());
        writer.bytes(&self.listing_id.0);
        writer.bytes(&self.review_key);
        writer.u32(self.paid_on);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            store_id: StoreId::from_bytes(reader.array()?),
            listing_id: ListingId(reader.array()?),
            review_key: reader.array()?,
            paid_on: reader.u32()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub receipt: SignedReceipt,
    /// 1 to 5 stars.
    pub rating: u8,
    pub text: String,
    /// YYYYMMDD, on or after the receipt's day.
    pub written_on: u32,
}

pub type SignedReview = Signed<Review>;

impl Terms for Review {
    const TAG: &'static str = "ob/review/v1";
    const MAGIC: [u8; 4] = *b"OBRV";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.receipt.terms.review_key
    }

    fn check(&self) -> Result<(), Error> {
        if !(1..=5).contains(&self.rating) {
            return Err(Error::BadTerms("a review is 1 to 5 stars"));
        }
        if self.text.len() > MAX_REVIEW_TEXT {
            return Err(Error::BadTerms("a review is at most 1,000 bytes"));
        }
        if self.written_on < self.receipt.terms.paid_on {
            return Err(Error::BadTerms("a review is written after the purchase"));
        }
        Ok(())
    }

    fn check_public_text(&self) -> Result<(), Error> {
        public_text::check("review", &self.text)
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.blob(&self.receipt.encode()?)?;
        writer.u8(self.rating);
        writer.str(&self.text)?;
        writer.u32(self.written_on);
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            receipt: SignedReceipt::decode(&reader.blob()?)?,
            rating: reader.u8()?,
            text: reader.str()?,
            written_on: reader.u32()?,
        })
    }
}

impl SignedReview {
    /// The review is signed by its receipt's key and the receipt by its store.
    pub fn verify_purchase(&self) -> Result<(), Error> {
        self.terms.receipt.verify()?;
        self.verify()
    }

    pub fn store_id(&self) -> StoreId {
        self.terms.receipt.terms.store_id
    }

    pub fn listing_id(&self) -> ListingId {
        self.terms.receipt.terms.listing_id
    }
}
