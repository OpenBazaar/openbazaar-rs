use bitcoin::secp256k1::SecretKey;

use crate::content::ContentRating;
use crate::encode::{self, Reader, Writer};
use crate::payment::DirectPay;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/store/v1";
const MAGIC: [u8; 4] = *b"OBST";
/// Signed before stores carried a rating. Still decoded and verified.
const VERSION_UNRATED: u8 = 1;
const VERSION: u8 = 2;

/// X-only store key. Listings name the store by this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreId([u8; PUBKEY_LEN]);

impl StoreId {
    pub(crate) fn from_bytes(bytes: [u8; PUBKEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        encode::to_hex(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Store {
    pub name: String,
    pub region: String,
    pub direct_pay: DirectPay,
    pub id: StoreId,
    /// A rating for everything the store sells, on top of each listing's
    /// own label. `None` only for a version 1 record.
    pub rating: Option<ContentRating>,
    pub signature: [u8; SIG_LEN],
}

impl Store {
    pub fn sign(
        name: impl Into<String>,
        region: impl Into<String>,
        direct_pay: DirectPay,
        rating: ContentRating,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        let store = Self {
            name: name.into(),
            region: region.into(),
            direct_pay,
            id: StoreId::from_bytes(sign::xonly(secret)),
            rating: Some(rating),
            signature: [0u8; SIG_LEN],
        };
        let body = store.encode_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self { signature, ..store })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut body = self.encode_body()?;
        body.extend_from_slice(&self.signature);
        Ok(body)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (store, len) = Self::decode_one(bytes)?;
        if len != bytes.len() {
            return Err(Error::TrailingBytes);
        }
        Ok(store)
    }

    /// Decode the first store record in `bytes`.
    ///
    /// Returns the record and how many bytes it occupied. Bytes after that
    /// belong to the next catalog record.
    pub(crate) fn decode_one(bytes: &[u8]) -> Result<(Self, usize), Error> {
        let mut reader = Reader::new(bytes);
        let store = Self::decode_body(&mut reader)?;
        let signature = reader.array()?;
        Ok((Self { signature, ..store }, reader.position()))
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, self.id.as_bytes(), &self.signature)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(match self.rating {
            Some(_) => VERSION,
            None => VERSION_UNRATED,
        });
        writer.str(&self.name)?;
        writer.str(&self.region)?;
        writer.str(&self.direct_pay.bolt12_offer)?;
        writer.str(&self.direct_pay.silent_payment_code)?;
        writer.bytes(self.id.as_bytes());
        if let Some(rating) = self.rating {
            writer.u8(rating.tag());
        }
        Ok(writer.finish())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let version = reader.expect_magic_in(&MAGIC, &[VERSION_UNRATED, VERSION])?;
        let mut store = Self {
            name: reader.str()?,
            region: reader.str()?,
            direct_pay: DirectPay {
                bolt12_offer: reader.str()?,
                silent_payment_code: reader.str()?,
            },
            id: StoreId::from_bytes(reader.array()?),
            rating: None,
            signature: [0u8; SIG_LEN],
        };
        if version == VERSION {
            store.rating = Some(ContentRating::from_tag(reader.u8()?)?);
        }
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Store {
        Store::sign(
            "Marlow Goods",
            "Oregon",
            DirectPay {
                bolt12_offer: "bolt12-offer:fixture:marlow-goods".to_string(),
                silent_payment_code: "bip352-code:fixture:marlow-goods".to_string(),
            },
            ContentRating::General,
            &sign::secret_from_label("store-test"),
        )
        .unwrap()
    }

    /// The fixture store as the version 1 code signed it, before ratings.
    const UNRATED_STORE: &str = "4f425354010c004d61726c6f7720476f6f647306004f7265676f6e2100626f6c7431322d6f666665723a666978747572653a6d61726c6f772d676f6f647320006269703335322d636f64653a666978747572653a6d61726c6f772d676f6f64732b489bf1506ec6e9fc3d968a519fcaf07ffe0394fdc313b513fc7d0ba4229056a70d3658e80be6a04bf99694daf664f22842d0860d82fb274a245ed59811a8ffb07c5e61ec90f020f02d698b161f5260dd9cfe8fb440d17da686d44c7b75891f";

    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn version_1_store_still_verifies_as_unrated() {
        let bytes = from_hex(UNRATED_STORE);
        let store = Store::decode(&bytes).unwrap();
        assert_eq!(store.rating, None);
        store.verify().unwrap();
        assert_eq!(store.encode().unwrap(), bytes);
    }

    #[test]
    fn rating_is_covered_by_the_signature() {
        let store = Store::sign(
            "Late Shop",
            "Nevada",
            sample().direct_pay,
            ContentRating::Adult,
            &sign::secret_from_label("store-test"),
        )
        .unwrap();
        let decoded = Store::decode(&store.encode().unwrap()).unwrap();
        assert_eq!(decoded.rating, Some(ContentRating::Adult));
        decoded.verify().unwrap();

        let mut softened = store.clone();
        softened.rating = Some(ContentRating::General);
        assert_eq!(softened.verify(), Err(Error::BadSignature));

        let mut stripped = store;
        stripped.rating = None;
        assert_eq!(stripped.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn round_trip_and_verify() {
        let store = sample();
        let bytes = store.encode().unwrap();
        let decoded = Store::decode(&bytes).unwrap();
        assert_eq!(decoded, store);
        decoded.verify().unwrap();
    }

    #[test]
    fn tampered_name_is_rejected() {
        let mut store = sample();
        store.name.push('!');
        assert_eq!(store.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let mut store = sample();
        store.signature[63] ^= 0x01;
        assert_eq!(store.verify(), Err(Error::BadSignature));
    }
}
