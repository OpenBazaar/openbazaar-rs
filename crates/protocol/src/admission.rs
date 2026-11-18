//! A store's admission to the directory.
//!
//! The head names the fee. It does not name stores. A store key signs this
//! record over the transaction that paid the fee. Introductions store the
//! bytes. A buyer still drops the record when the chain check fails.
//!
//! Wire format (integers little-endian):
//!
//! ```text
//! body = "OBAD" | version u8 | store id [32] | txid [32] | vout u32 | height u32
//! doc  = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/admit/v{version}` of
//! `body`, under the store id. 4 + 1 + 32 + 32 + 4 + 4 + 64 = 141 bytes.
//!
//! Version 1 pays the head's fee key and names the store in an OP_RETURN of
//! `OBAD` and the store id. Version 2 pays [`store_fee_key`], which names the
//! store by itself, so any wallet, exchange, or swap that pays an address can
//! pay the fee.

use bitcoin::secp256k1::{Keypair, Scalar, Secp256k1, SecretKey, XOnlyPublicKey};

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const MAGIC: [u8; 4] = *b"OBAD";
const VERSION: u8 = 2;
const FEE_KEY_TAG: &str = "ob/fee-key/v1";
/// Encoded size of one admission, signature included.
pub const ADMISSION_LEN: usize = 4 + 1 + 32 + 32 + 4 + 4 + 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
    /// 1 for a payment to the head's fee key with an OP_RETURN, 2 for a payment to [`store_fee_key`].
    pub version: u8,
    pub store_id: [u8; PUBKEY_LEN],
    /// Consensus byte order, the same order [`bitcoin::Txid::to_byte_array`] uses.
    pub txid: [u8; 32],
    pub vout: u32,
    /// Height of the block that confirmed the fee.
    pub height: u32,
    pub signature: [u8; SIG_LEN],
}

impl Admission {
    pub fn sign(
        store_id: [u8; PUBKEY_LEN],
        txid: [u8; 32],
        vout: u32,
        height: u32,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        Self::sign_version(VERSION, store_id, txid, vout, height, secret)
    }

    /// A version 1 admission, as nodes signed before [`store_fee_key`].
    pub fn sign_v1(
        store_id: [u8; PUBKEY_LEN],
        txid: [u8; 32],
        vout: u32,
        height: u32,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        Self::sign_version(1, store_id, txid, vout, height, secret)
    }

    fn sign_version(
        version: u8,
        store_id: [u8; PUBKEY_LEN],
        txid: [u8; 32],
        vout: u32,
        height: u32,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        if sign::xonly(secret) != store_id {
            return Err(Error::StoreMismatch);
        }
        let admission = Self { version, store_id, txid, vout, height, signature: [0u8; SIG_LEN] };
        let body = admission.encode_body()?;
        let signature = sign::sign(&tag(version), &body, secret);
        Ok(Self { signature, ..admission })
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(&tag(self.version), &body, &self.store_id, &self.signature)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = self.encode_body()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != ADMISSION_LEN {
            return Err(Error::BadSeed("an admission is 141 bytes"));
        }
        let mut reader = Reader::new(bytes);
        let version = match bytes.get(4) {
            Some(1) => 1,
            _ => VERSION,
        };
        reader.expect_magic(&MAGIC, version)?;
        let store_id = reader.array()?;
        let txid = reader.array()?;
        let vout = reader.u32()?;
        let height = reader.u32()?;
        let signature = reader.array()?;
        reader.finish()?;
        let admission = Self { version, store_id, txid, vout, height, signature };
        admission.verify()?;
        Ok(admission)
    }

    /// The store id at its fixed offset, even when the signature does not verify.
    pub fn peek_store_id(bytes: &[u8]) -> Option<[u8; PUBKEY_LEN]> {
        if bytes.len() != ADMISSION_LEN || bytes.get(..4) != Some(&MAGIC) || !matches!(bytes.get(4).copied(), Some(1 | VERSION)) {
            return None;
        }
        bytes.get(5..37).and_then(|id| id.try_into().ok())
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        if !matches!(self.version, 1 | VERSION) {
            return Err(Error::BadVersion);
        }
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(self.version);
        writer.bytes(&self.store_id);
        writer.bytes(&self.txid);
        writer.u32(self.vout);
        writer.u32(self.height);
        Ok(writer.finish())
    }
}

fn tag(version: u8) -> String {
    format!("ob/admit/v{version}")
}

fn fee_tweak(fee_key: &[u8; PUBKEY_LEN], store_id: &[u8; PUBKEY_LEN]) -> Result<Scalar, Error> {
    let mut body = [0u8; 2 * PUBKEY_LEN];
    body[..PUBKEY_LEN].copy_from_slice(fee_key);
    body[PUBKEY_LEN..].copy_from_slice(store_id);
    Scalar::from_be_bytes(sign::tagged_hash(FEE_KEY_TAG, &body)).map_err(|_| Error::BadKey)
}

/// The x-only key a version 2 fee for `store_id` pays, as a key-path-only
/// Taproot output: `fee_key + t·G` with `t = tagged_hash("ob/fee-key/v1",
/// fee_key || store_id)`. Anyone can compute it from the head and the store id.
pub fn store_fee_key(fee_key: &[u8; PUBKEY_LEN], store_id: &[u8; PUBKEY_LEN]) -> Result<[u8; PUBKEY_LEN], Error> {
    let key = XOnlyPublicKey::from_slice(fee_key).map_err(|_| Error::BadKey)?;
    let (tweaked, _) = key.add_tweak(&Secp256k1::verification_only(), &fee_tweak(fee_key, store_id)?).map_err(|_| Error::BadKey)?;
    Ok(tweaked.serialize())
}

/// The secret for [`store_fee_key`], from the secret of the head's fee key.
/// Only the directory operator holds that secret.
pub fn store_fee_secret(fee_secret: &SecretKey, store_id: &[u8; PUBKEY_LEN]) -> Result<SecretKey, Error> {
    let secp = Secp256k1::new();
    let tweak = fee_tweak(&sign::xonly(fee_secret), store_id)?;
    let keypair = Keypair::from_secret_key(&secp, fee_secret).add_xonly_tweak(&secp, &tweak).map_err(|_| Error::BadKey)?;
    Ok(keypair.secret_key())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::secret_from_label;

    #[test]
    fn an_admission_is_141_bytes_and_a_flipped_byte_fails() {
        let secret = secret_from_label("admit-test");
        let id = sign::xonly(&secret);
        let admission = Admission::sign(id, [9u8; 32], 1, 100, &secret).unwrap();
        assert_eq!(admission.version, 2);
        let bytes = admission.encode().unwrap();
        assert_eq!(bytes.len(), ADMISSION_LEN);
        assert_eq!(Admission::decode(&bytes).unwrap(), admission);
        let mut flipped = bytes.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0x01;
        assert!(Admission::decode(&flipped).is_err());
        assert_eq!(Admission::peek_store_id(&flipped), Some(id));
        let other = secret_from_label("other-store");
        assert!(Admission::sign(id, [9u8; 32], 0, 1, &other).is_err());
    }

    #[test]
    fn a_version_1_admission_still_decodes_and_cannot_pass_as_version_2() {
        let secret = secret_from_label("admit-v1");
        let id = sign::xonly(&secret);
        let old = Admission::sign_v1(id, [3u8; 32], 0, 50, &secret).unwrap();
        let bytes = old.encode().unwrap();
        assert_eq!(bytes[4], 1);
        assert_eq!(Admission::decode(&bytes).unwrap(), old);
        assert_eq!(Admission::peek_store_id(&bytes), Some(id));
        let mut relabeled = bytes.clone();
        relabeled[4] = 2;
        assert!(Admission::decode(&relabeled).is_err(), "a version 1 signature does not verify as version 2");
        let mut unknown = bytes;
        unknown[4] = 3;
        assert!(Admission::decode(&unknown).is_err());
        assert_eq!(Admission::peek_store_id(&unknown), None);
    }

    #[test]
    fn the_operator_secret_spends_each_store_fee_key_and_stores_differ() {
        let operator = secret_from_label("fee-operator");
        let fee_key = sign::xonly(&operator);
        let cedar = sign::xonly(&secret_from_label("cedar"));
        let birch = sign::xonly(&secret_from_label("birch"));
        let cedar_key = store_fee_key(&fee_key, &cedar).unwrap();
        assert_ne!(cedar_key, fee_key);
        assert_ne!(cedar_key, store_fee_key(&fee_key, &birch).unwrap(), "one store's fee key does not admit another");
        assert_eq!(sign::xonly(&store_fee_secret(&operator, &cedar).unwrap()), cedar_key);
        assert_eq!(sign::xonly(&store_fee_secret(&operator, &birch).unwrap()), store_fee_key(&fee_key, &birch).unwrap());
        assert!(store_fee_key(&[0u8; 32], &cedar).is_err());
    }

    #[test]
    fn twenty_thousand_admissions_are_three_pages() {
        const PAGE: usize = 1_048_576;
        assert_eq!(20_000 * ADMISSION_LEN, 2_820_000);
        assert_eq!(PAGE / ADMISSION_LEN, 7_436);
        let pages = 20_000_usize.div_ceil(PAGE / ADMISSION_LEN);
        assert_eq!(pages, 3);
    }
}
