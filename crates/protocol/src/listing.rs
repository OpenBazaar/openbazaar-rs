use bitcoin::secp256k1::SecretKey;

use crate::content::{ContentLabel, ContentRating, ContentTags};
use crate::encode::{self, Reader, Writer};
use crate::sign::{self, SIG_LEN};
use crate::store::StoreId;
use crate::Error;

const TAG: &str = "ob/listing/v1";
const MAGIC: [u8; 4] = *b"OBLS";
/// Signed before content labels existed. Still decoded and verified.
const VERSION_UNLABELED: u8 = 4;
const VERSION: u8 = 5;

/// Address of a listing: SHA256 of the canonical body, without the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListingHash([u8; 32]);

impl ListingHash {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        encode::to_hex(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingKind {
    Physical,
    Digital,
}

impl ListingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ListingKind::Physical => "physical",
            ListingKind::Digital => "digital",
        }
    }

    fn tag(self) -> u8 {
        match self {
            ListingKind::Physical => 0,
            ListingKind::Digital => 1,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, Error> {
        match tag {
            0 => Ok(ListingKind::Physical),
            1 => Ok(ListingKind::Digital),
            _ => Err(Error::UnknownKind),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub store_id: StoreId,
    pub title: String,
    pub category: String,
    pub price_sats: u64,
    pub ship_from: String,
    pub description: String,
    pub seller: String,
    pub stock: u32,
    /// Calendar day the listing was signed, as YYYYMMDD.
    pub listed_on: u32,
    pub moderated: bool,
    pub kind: ListingKind,
    /// Shared product key. Empty when this listing is the only offer.
    pub product: String,
    /// Flat shipping for one physical package from this seller, in sats.
    pub shipping_sats: u64,
    /// Item subtotal, in sats, at which that flat shipping drops to zero. Zero means it never does.
    pub free_over_sats: u64,
    /// The vendor's own label. `None` only for a version 4 record, which
    /// was signed before labels existed and is unrated, not general.
    pub content: Option<ContentLabel>,
    pub signature: [u8; SIG_LEN],
}

impl Listing {
    pub fn sign(
        store_id: StoreId,
        title: impl Into<String>,
        category: impl Into<String>,
        price_sats: u64,
        ship_from: impl Into<String>,
        description: impl Into<String>,
        seller: impl Into<String>,
        stock: u32,
        listed_on: u32,
        moderated: bool,
        kind: ListingKind,
        product: impl Into<String>,
        shipping_sats: u64,
        free_over_sats: u64,
        content: ContentLabel,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        if sign::xonly(secret) != *store_id.as_bytes() {
            return Err(Error::StoreMismatch);
        }
        let listing = Self {
            store_id,
            title: title.into(),
            category: category.into(),
            price_sats,
            ship_from: ship_from.into(),
            description: description.into(),
            seller: seller.into(),
            stock,
            listed_on,
            moderated,
            kind,
            product: product.into(),
            shipping_sats,
            free_over_sats,
            content: Some(content),
            signature: [0u8; SIG_LEN],
        };
        let body = listing.encode_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self {
            signature,
            ..listing
        })
    }

    pub fn content_hash(&self) -> Result<ListingHash, Error> {
        Ok(ListingHash::from_bytes(sign::content_hash(
            &self.encode_body()?,
        )))
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut body = self.encode_body()?;
        body.extend_from_slice(&self.signature);
        Ok(body)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (listing, len) = Self::decode_one(bytes)?;
        if len != bytes.len() {
            return Err(Error::TrailingBytes);
        }
        Ok(listing)
    }

    /// Decode the first listing record in `bytes`.
    ///
    /// Returns the record and how many bytes it occupied.
    pub(crate) fn decode_one(bytes: &[u8]) -> Result<(Self, usize), Error> {
        let mut reader = Reader::new(bytes);
        let listing = Self::decode_body(&mut reader)?;
        let signature = reader.array()?;
        Ok((
            Self {
                signature,
                ..listing
            },
            reader.position(),
        ))
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, self.store_id.as_bytes(), &self.signature)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(match self.content {
            Some(_) => VERSION,
            None => VERSION_UNLABELED,
        });
        writer.bytes(self.store_id.as_bytes());
        writer.str(&self.title)?;
        writer.str(&self.category)?;
        writer.u64(self.price_sats);
        writer.str(&self.ship_from)?;
        writer.str(&self.description)?;
        writer.str(&self.seller)?;
        writer.u32(self.stock);
        writer.u32(self.listed_on);
        writer.u8(u8::from(self.moderated));
        writer.u8(self.kind.tag());
        writer.str(&self.product)?;
        writer.u64(self.shipping_sats);
        writer.u64(self.free_over_sats);
        if let Some(content) = self.content {
            writer.u8(content.rating.tag());
            writer.u16(content.tags.bits());
        }
        Ok(writer.finish())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let version = reader.expect_magic_in(&MAGIC, &[VERSION_UNLABELED, VERSION])?;
        let mut listing = Self {
            store_id: StoreId::from_bytes(reader.array()?),
            title: reader.str()?,
            category: reader.str()?,
            price_sats: reader.u64()?,
            ship_from: reader.str()?,
            description: reader.str()?,
            seller: reader.str()?,
            stock: reader.u32()?,
            listed_on: reader.u32()?,
            moderated: reader.u8()? == 1,
            kind: ListingKind::from_tag(reader.u8()?)?,
            product: reader.str()?,
            shipping_sats: reader.u64()?,
            free_over_sats: reader.u64()?,
            content: None,
            signature: [0u8; SIG_LEN],
        };
        if version == VERSION {
            listing.content = Some(ContentLabel {
                rating: ContentRating::from_tag(reader.u8()?)?,
                tags: ContentTags::from_bits(reader.u16()?),
            });
        }
        Ok(listing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payment::DirectPay;
    use crate::store::Store;

    fn sample() -> Listing {
        sample_with(ContentLabel::GENERAL)
    }

    fn sample_with(content: ContentLabel) -> Listing {
        let secret = sign::secret_from_label("listing-test");
        let store = Store::sign(
            "Marlow Goods",
            "Oregon",
            DirectPay {
                bolt12_offer: "bolt12-offer:fixture".to_string(),
                silent_payment_code: "bip352-code:fixture".to_string(),
            },
            ContentRating::General,
            &secret,
        )
        .unwrap();
        Listing::sign(
            store.id,
            "Paperback novel",
            "Books",
            18_000,
            "Oregon",
            "A sewn paperback from a small press. It is packed flat so the spine stays straight.",
            "Marlow Goods",
            8,
            20260312,
            true,
            ListingKind::Physical,
            "",
            800,
            30_000,
            content,
            &secret,
        )
        .unwrap()
    }

    fn adult() -> ContentLabel {
        ContentLabel::new(
            ContentRating::Adult,
            ContentTags::NUDITY.with(ContentTags::DRUGS),
        )
    }

    /// "Dip pen" from the fixture shop, as the version 4 code signed it.
    const UNLABELED_LISTING: &str = "4f424c53042b489bf1506ec6e9fc3d968a519fcaf07ffe0394fdc313b513fc7d0ba422905607004469702070656e0500546f6f6c73781e00000000000007005665726d6f6e7440004120737465656c206e696220616e6420612073686f727420776f6f6420686f6c6465722e20497420736869707320696e206120706170657220736c656576652e0a0048616c652050617065720600000003273501010000005802000000000000204e000000000000934986a799cdfad597f8b6d984c75e0146f72270aa36a0cd228479295a998b5f103e4d6136831260f98e25d9d7ae9370a886db58ab15f1d0c49f74d1b8af2332";
    const UNLABELED_HASH: &str = "9344117bf6a09a44e8f193f6ee9bd687602b38b569f5b3239b4a3106bb0e4471";

    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Offset of the rating byte in an encoded labeled listing.
    fn rating_offset(bytes: &[u8]) -> usize {
        bytes.len() - SIG_LEN - 3
    }

    #[test]
    fn label_round_trips_inside_the_signed_body() {
        let listing = sample_with(adult());
        let bytes = listing.encode().unwrap();
        assert_eq!(bytes[4], VERSION);
        let decoded = Listing::decode(&bytes).unwrap();
        assert_eq!(decoded.content, Some(adult()));
        decoded.verify().unwrap();
        assert_ne!(
            decoded.content_hash().unwrap(),
            sample().content_hash().unwrap()
        );
    }

    #[test]
    fn changing_the_rating_fails_verify() {
        let listing = sample_with(adult());
        let mut softened = listing.clone();
        softened.content = Some(ContentLabel::new(ContentRating::General, adult().tags));
        assert_ne!(
            softened.content_hash().unwrap(),
            listing.content_hash().unwrap()
        );
        assert_eq!(softened.verify(), Err(Error::BadSignature));

        let mut raised = sample();
        raised.content = Some(adult());
        assert_eq!(raised.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn changing_the_tags_fails_verify() {
        let mut listing = sample_with(adult());
        listing.content = Some(ContentLabel::new(ContentRating::Adult, ContentTags::DRUGS));
        assert_eq!(listing.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn stripping_the_label_fails_verify() {
        let mut listing = sample_with(adult());
        listing.content = None;
        assert_eq!(listing.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn relay_edits_to_the_label_bytes_fail_verify() {
        let bytes = sample_with(adult()).encode().unwrap();

        let mut softened = bytes.clone();
        let at = rating_offset(&softened);
        assert_eq!(softened[at], ContentRating::Adult.tag());
        softened[at] = ContentRating::General.tag();
        let decoded = Listing::decode(&softened).unwrap();
        assert_eq!(decoded.content.unwrap().rating, ContentRating::General);
        assert_eq!(decoded.verify(), Err(Error::BadSignature));

        let mut untagged = bytes.clone();
        untagged[at + 1] = 0;
        untagged[at + 2] = 0;
        let decoded = Listing::decode(&untagged).unwrap();
        assert_eq!(decoded.verify(), Err(Error::BadSignature));

        // Drop the label and claim version 4, as if it was never there.
        let mut downgraded = bytes.clone();
        downgraded.drain(at..at + 3);
        downgraded[4] = VERSION_UNLABELED;
        let decoded = Listing::decode(&downgraded).unwrap();
        assert_eq!(decoded.content, None);
        assert_eq!(decoded.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn unknown_rating_byte_does_not_decode() {
        let mut bytes = sample().encode().unwrap();
        let at = rating_offset(&bytes);
        bytes[at] = 9;
        assert_eq!(Listing::decode(&bytes), Err(Error::UnknownRating));
    }

    #[test]
    fn version_4_listing_still_verifies_as_unrated() {
        let bytes = from_hex(UNLABELED_LISTING);
        let listing = Listing::decode(&bytes).unwrap();
        assert_eq!(listing.title, "Dip pen");
        assert_eq!(listing.content, None);
        listing.verify().unwrap();
        assert_eq!(listing.content_hash().unwrap().to_hex(), UNLABELED_HASH);
        assert_eq!(listing.encode().unwrap(), bytes);

        let mut labeled = listing;
        labeled.content = Some(ContentLabel::GENERAL);
        assert_eq!(labeled.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn round_trip_hash_and_verify() {
        let listing = sample();
        let bytes = listing.encode().unwrap();
        let decoded = Listing::decode(&bytes).unwrap();
        assert_eq!(decoded, listing);
        decoded.verify().unwrap();
        assert_eq!(
            decoded.content_hash().unwrap(),
            listing.content_hash().unwrap()
        );
        assert_eq!(decoded.encode().unwrap(), bytes);
    }

    #[test]
    fn tampered_title_changes_the_address_and_fails_verify() {
        let listing = sample();
        let original = listing.content_hash().unwrap();
        let mut tampered = listing.clone();
        tampered.title.push('!');
        assert_ne!(tampered.content_hash().unwrap(), original);
        assert_eq!(tampered.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let mut listing = sample();
        listing.signature[0] ^= 0xff;
        assert_eq!(listing.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn wrong_store_key_cannot_sign() {
        let secret = sign::secret_from_label("listing-test");
        let other = sign::secret_from_label("someone-else");
        let store = Store::sign(
            "Marlow Goods",
            "Oregon",
            DirectPay {
                bolt12_offer: "bolt12-offer:fixture".to_string(),
                silent_payment_code: "bip352-code:fixture".to_string(),
            },
            ContentRating::General,
            &secret,
        )
        .unwrap();
        let result = Listing::sign(
            store.id,
            "Paperback novel",
            "Books",
            18_000,
            "Oregon",
            "A sewn paperback from a small press.",
            "Marlow Goods",
            8,
            20260312,
            true,
            ListingKind::Physical,
            "",
            800,
            30_000,
            ContentLabel::GENERAL,
            &other,
        );
        assert_eq!(result.err(), Some(Error::StoreMismatch));
    }
}
