use bitcoin::secp256k1::SecretKey;

use crate::encode::{self, Reader, Writer};
use crate::sign::{self, SIG_LEN};
use crate::store::StoreId;
use crate::Error;

const TAG: &str = "ob/listing/v1";
const MAGIC: [u8; 4] = *b"OBLS";
const VERSION: u8 = 1;

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
    pub kind: ListingKind,
    pub signature: [u8; SIG_LEN],
}

impl Listing {
    pub fn sign(
        store_id: StoreId,
        title: impl Into<String>,
        category: impl Into<String>,
        price_sats: u64,
        ship_from: impl Into<String>,
        kind: ListingKind,
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
            kind,
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
        let mut reader = Reader::new(bytes);
        let listing = Self::decode_body(&mut reader)?;
        let signature = reader.array()?;
        reader.finish()?;
        Ok(Self {
            signature,
            ..listing
        })
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, self.store_id.as_bytes(), &self.signature)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(self.store_id.as_bytes());
        writer.str(&self.title)?;
        writer.str(&self.category)?;
        writer.u64(self.price_sats);
        writer.str(&self.ship_from)?;
        writer.u8(self.kind.tag());
        Ok(writer.finish())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.expect_magic(&MAGIC, VERSION)?;
        Ok(Self {
            store_id: StoreId::from_bytes(reader.array()?),
            title: reader.str()?,
            category: reader.str()?,
            price_sats: reader.u64()?,
            ship_from: reader.str()?,
            kind: ListingKind::from_tag(reader.u8()?)?,
            signature: [0u8; SIG_LEN],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payment::DirectPay;
    use crate::store::Store;

    fn sample() -> Listing {
        let secret = sign::secret_from_label("listing-test");
        let store = Store::sign(
            "Marlow Goods",
            "Oregon",
            DirectPay {
                bolt12_offer: "bolt12-offer:fixture".to_string(),
                silent_payment_code: "bip352-code:fixture".to_string(),
            },
            &secret,
        )
        .unwrap();
        Listing::sign(
            store.id,
            "Paperback novel",
            "Books",
            18_000,
            "Oregon",
            ListingKind::Physical,
            &secret,
        )
        .unwrap()
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
            &secret,
        )
        .unwrap();
        let result = Listing::sign(
            store.id,
            "Paperback novel",
            "Books",
            18_000,
            "Oregon",
            ListingKind::Physical,
            &other,
        );
        assert_eq!(result.err(), Some(Error::StoreMismatch));
    }
}
