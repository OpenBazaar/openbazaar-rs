//! A store's admission to the directory.
//!
//! The head names the fee. It does not name stores. A store key signs this
//! record over the transaction that paid the fee. Introductions store the
//! bytes. A buyer still drops the record when the chain check fails.
//!
//! Wire format (integers little-endian):
//!
//! ```text
//! body = "OBAD" | 0x01 | store id [32] | txid [32] | vout u32 | height u32
//! doc  = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/admit/v1` of `body`,
//! under the store id. 4 + 1 + 32 + 32 + 4 + 4 + 64 = 141 bytes.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/admit/v1";
const MAGIC: [u8; 4] = *b"OBAD";
const VERSION: u8 = 1;
/// Encoded size of one admission, signature included.
pub const ADMISSION_LEN: usize = 4 + 1 + 32 + 32 + 4 + 4 + 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Admission {
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
        if sign::xonly(secret) != store_id {
            return Err(Error::StoreMismatch);
        }
        let admission = Self { store_id, txid, vout, height, signature: [0u8; SIG_LEN] };
        let body = admission.encode_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self { signature, ..admission })
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, &self.store_id, &self.signature)
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
        reader.expect_magic(&MAGIC, VERSION)?;
        let store_id = reader.array()?;
        let txid = reader.array()?;
        let vout = reader.u32()?;
        let height = reader.u32()?;
        let signature = reader.array()?;
        reader.finish()?;
        let admission = Self { store_id, txid, vout, height, signature };
        admission.verify()?;
        Ok(admission)
    }

    /// The store id at its fixed offset, even when the signature does not verify.
    pub fn peek_store_id(bytes: &[u8]) -> Option<[u8; PUBKEY_LEN]> {
        if bytes.len() != ADMISSION_LEN || bytes.get(..4) != Some(&MAGIC) || bytes.get(4) != Some(&VERSION) {
            return None;
        }
        bytes.get(5..37).and_then(|id| id.try_into().ok())
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.store_id);
        writer.bytes(&self.txid);
        writer.u32(self.vout);
        writer.u32(self.height);
        Ok(writer.finish())
    }
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
    fn twenty_thousand_admissions_are_three_pages() {
        const PAGE: usize = 1_048_576;
        assert_eq!(20_000 * ADMISSION_LEN, 2_820_000);
        assert_eq!(PAGE / ADMISSION_LEN, 7_436);
        let pages = 20_000_usize.div_ceil(PAGE / ADMISSION_LEN);
        assert_eq!(pages, 3);
    }
}
