//! Best offer: a buyer's bid and the seller's signed answer.
//!
//! The bid travels inside a sealed order letter, signed by that order's
//! conversation key, so it does not link to the buyer's other orders. The
//! seller answers with an [`Offer`] signed by the store key and bound to
//! that conversation key: a buyer can check it offline, and only that buyer
//! can order with it. A counter from the buyer is a new bid. The seller's
//! floor and automatic rules never leave the seller's node.

use crate::encode::{Reader, Writer};
use crate::listing_details::ListingId;
use crate::signed::{check_currency, Signed, Terms};
use crate::sign::PUBKEY_LEN;
use crate::store::StoreId;
use crate::variants::VariantId;
use crate::Error;

const BID_MAGIC: [u8; 4] = *b"OBBD";
const BID_VERSION: u8 = 1;
const MAX_QUANTITY: u16 = 999;

/// What a buyer proposes. Not signed on its own: the letter carrying it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bid {
    pub listing_id: ListingId,
    pub variant_id: VariantId,
    pub quantity: u16,
    /// Per unit, in minor units of `currency`.
    pub amount: u64,
    pub currency: String,
}

impl Bid {
    fn check(&self) -> Result<(), Error> {
        check_terms(self.quantity, self.amount)?;
        check_currency(&self.currency)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.check()?;
        let mut writer = Writer::new();
        writer.bytes(&BID_MAGIC);
        writer.u8(BID_VERSION);
        writer.bytes(&self.listing_id.0);
        writer.bytes(&self.variant_id.0);
        writer.u16(self.quantity);
        writer.u64(self.amount);
        writer.str(&self.currency)?;
        Ok(writer.finish())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(&BID_MAGIC, BID_VERSION)?;
        let bid = Self {
            listing_id: ListingId(reader.array()?),
            variant_id: VariantId(reader.array()?),
            quantity: reader.u16()?,
            amount: reader.u64()?,
            currency: reader.str()?,
        };
        reader.finish()?;
        bid.check()?;
        Ok(bid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub store_id: StoreId,
    pub offer_id: [u8; 16],
    pub listing_id: ListingId,
    /// [`VariantId::NONE`] for a listing without variants.
    pub variant_id: VariantId,
    pub quantity: u16,
    /// Per unit, in minor units of `currency`.
    pub amount: u64,
    pub currency: String,
    /// The conversation key of the bid this answers. Only it can use the offer.
    pub buyer_key: [u8; PUBKEY_LEN],
    /// Unix seconds.
    pub expires_at: u64,
    /// True when the seller proposes a different amount than the bid.
    pub counter: bool,
}

pub type SignedOffer = Signed<Offer>;

impl Offer {
    pub fn live_at(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

impl Terms for Offer {
    const TAG: &'static str = "ob/offer/v1";
    const MAGIC: [u8; 4] = *b"OBOF";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        *self.store_id.as_bytes()
    }

    fn check(&self) -> Result<(), Error> {
        check_terms(self.quantity, self.amount)?;
        check_currency(&self.currency)
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(self.store_id.as_bytes());
        writer.bytes(&self.offer_id);
        writer.bytes(&self.listing_id.0);
        writer.bytes(&self.variant_id.0);
        writer.u16(self.quantity);
        writer.u64(self.amount);
        writer.str(&self.currency)?;
        writer.bytes(&self.buyer_key);
        writer.u64(self.expires_at);
        writer.u8(u8::from(self.counter));
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            store_id: StoreId::from_bytes(reader.array()?),
            offer_id: reader.array()?,
            listing_id: ListingId(reader.array()?),
            variant_id: VariantId(reader.array()?),
            quantity: reader.u16()?,
            amount: reader.u64()?,
            currency: reader.str()?,
            buyer_key: reader.array()?,
            expires_at: reader.u64()?,
            counter: match reader.u8()? {
                0 => false,
                1 => true,
                _ => return Err(Error::BadTerms("unknown counter flag")),
            },
        })
    }
}

fn check_terms(quantity: u16, amount: u64) -> Result<(), Error> {
    if quantity == 0 || quantity > MAX_QUANTITY {
        return Err(Error::BadTerms("an offer is for 1 to 999 units"));
    }
    if amount == 0 {
        return Err(Error::BadTerms("an offer is for more than zero"));
    }
    Ok(())
}
