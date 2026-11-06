use bitcoin::secp256k1::SecretKey;

use crate::encode::{self, Reader, Writer};
use crate::payment::DirectPay;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/store/v1";
const MAGIC: [u8; 4] = *b"OBST";
const VERSION: u8 = 1;

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
    pub signature: [u8; SIG_LEN],
}

impl Store {
    pub fn sign(
        name: impl Into<String>,
        region: impl Into<String>,
        direct_pay: DirectPay,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        let store = Self {
            name: name.into(),
            region: region.into(),
            direct_pay,
            id: StoreId::from_bytes(sign::xonly(secret)),
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
        let mut reader = Reader::new(bytes);
        let store = Self::decode_body(&mut reader)?;
        let signature = reader.array()?;
        reader.finish()?;
        Ok(Self { signature, ..store })
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, self.id.as_bytes(), &self.signature)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.str(&self.name)?;
        writer.str(&self.region)?;
        writer.str(&self.direct_pay.bolt12_offer)?;
        writer.str(&self.direct_pay.silent_payment_code)?;
        writer.bytes(self.id.as_bytes());
        Ok(writer.finish())
    }

    fn decode_body(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.expect_magic(&MAGIC, VERSION)?;
        Ok(Self {
            name: reader.str()?,
            region: reader.str()?,
            direct_pay: DirectPay {
                bolt12_offer: reader.str()?,
                silent_payment_code: reader.str()?,
            },
            id: StoreId::from_bytes(reader.array()?),
            signature: [0u8; SIG_LEN],
        })
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
            &sign::secret_from_label("store-test"),
        )
        .unwrap()
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
