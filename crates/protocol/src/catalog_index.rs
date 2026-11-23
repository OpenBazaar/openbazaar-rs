//! A store's signed catalog index, and the directory's signed admission snapshot.
//!
//! A replica can omit a listing or keep serving one the store retracted. The
//! store key signs the complete set of listing content hashes. A buyer that
//! holds a newer index for the same store record drops anything the index
//! does not name.
//!
//! The directory head names introductions and a fee schedule, not stores. The
//! directory key signs the store ids it currently admits. A buyer asks for
//! those records even when no one handed it the store. An admission whose
//! chain check fails is still dropped: the snapshot does not override that.
//!
//! Neither record carries free text. Listing hashes are the existing content
//! addresses (SHA-256 of the signed listing body), sorted, with no repeats.
//! The record digest is SHA-256 of the store's encoded signed record.
//!
//! ```text
//! catalog body = "OBCX" | 0x01 | store id [32] | sequence u64 | signed_on u32
//!              | record digest [32] | listing_count u16 | listing hash [32] * count
//! admit body   = "OBAX" | 0x01 | directory signer [32] | sequence u64 | signed_on u32
//!              | head sequence u64 | admitted_count u16 | store id [32] * count
//! doc          = body | signature [64]
//! ```
//!
//! Tags `ob/catalog-index/v1` and `ob/admission-index/v1`. One index lists at
//! most 4,096 ids, the same cap as a label set, so the reply stays one fetch.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const CATALOG_MAGIC: [u8; 4] = *b"OBCX";
const ADMISSION_MAGIC: [u8; 4] = *b"OBAX";
const VERSION: u8 = 1;
const CATALOG_TAG: &str = "ob/catalog-index/v1";
const ADMISSION_TAG: &str = "ob/admission-index/v1";
/// Listing hashes in one catalog index, and store ids in one admission snapshot.
pub const MAX_INDEX_ENTRIES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogIndex {
    pub store_id: [u8; PUBKEY_LEN],
    pub sequence: u64,
    pub signed_on: u32,
    pub record_digest: [u8; 32],
    /// Sorted, unique listing content hashes.
    pub listings: Vec<[u8; 32]>,
    pub signature: [u8; SIG_LEN],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionIndex {
    pub signer: [u8; PUBKEY_LEN],
    pub sequence: u64,
    pub signed_on: u32,
    /// Sequence of the directory head this snapshot accompanies.
    pub head_sequence: u64,
    /// Sorted, unique store ids.
    pub stores: Vec<[u8; 32]>,
    pub signature: [u8; SIG_LEN],
}

impl CatalogIndex {
    /// Sort `listings` into the one encoding and sign it with the store key.
    pub fn sign(
        sequence: u64,
        signed_on: u32,
        record_digest: [u8; 32],
        mut listings: Vec<[u8; 32]>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        listings.sort();
        let index = Self {
            store_id: sign::xonly(secret),
            sequence,
            signed_on,
            record_digest,
            listings,
            signature: [0u8; SIG_LEN],
        };
        index.check_shape()?;
        let body = index.encode_body()?;
        let signature = sign::sign(CATALOG_TAG, &body, secret);
        Ok(Self { signature, ..index })
    }

    pub fn verify(&self) -> Result<(), Error> {
        self.check_shape()?;
        let body = self.encode_body()?;
        sign::verify(CATALOG_TAG, &body, &self.store_id, &self.signature)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.check_shape()?;
        let mut bytes = self.encode_body()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(&CATALOG_MAGIC, VERSION)?;
        let store_id = reader.array()?;
        let sequence = reader.u64()?;
        let signed_on = reader.u32()?;
        let record_digest = reader.array()?;
        let listings = read_ids(&mut reader)?;
        let signature = reader.array()?;
        reader.finish()?;
        let index = Self {
            store_id,
            sequence,
            signed_on,
            record_digest,
            listings,
            signature,
        };
        index.check_shape()?;
        Ok(index)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&CATALOG_MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.store_id);
        writer.u64(self.sequence);
        writer.u32(self.signed_on);
        writer.bytes(&self.record_digest);
        write_ids(&mut writer, &self.listings)?;
        Ok(writer.finish())
    }

    fn check_shape(&self) -> Result<(), Error> {
        check_ids(&self.listings, "a catalog index")
    }
}

impl AdmissionIndex {
    /// Sort `stores` into the one encoding and sign it with the directory key.
    pub fn sign(
        sequence: u64,
        signed_on: u32,
        head_sequence: u64,
        mut stores: Vec<[u8; 32]>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        stores.sort();
        let index = Self {
            signer: sign::xonly(secret),
            sequence,
            signed_on,
            head_sequence,
            stores,
            signature: [0u8; SIG_LEN],
        };
        index.check_shape()?;
        let body = index.encode_body()?;
        let signature = sign::sign(ADMISSION_TAG, &body, secret);
        Ok(Self { signature, ..index })
    }

    pub fn verify(&self) -> Result<(), Error> {
        self.check_shape()?;
        let body = self.encode_body()?;
        sign::verify(ADMISSION_TAG, &body, &self.signer, &self.signature)
    }

    /// Verify, and require the signer to be the directory key `expected`.
    pub fn verify_signer(&self, expected: &[u8; PUBKEY_LEN]) -> Result<(), Error> {
        if self.signer != *expected {
            return Err(Error::BadSignature);
        }
        self.verify()
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.check_shape()?;
        let mut bytes = self.encode_body()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(&ADMISSION_MAGIC, VERSION)?;
        let signer = reader.array()?;
        let sequence = reader.u64()?;
        let signed_on = reader.u32()?;
        let head_sequence = reader.u64()?;
        let stores = read_ids(&mut reader)?;
        let signature = reader.array()?;
        reader.finish()?;
        let index = Self {
            signer,
            sequence,
            signed_on,
            head_sequence,
            stores,
            signature,
        };
        index.check_shape()?;
        Ok(index)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&ADMISSION_MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.signer);
        writer.u64(self.sequence);
        writer.u32(self.signed_on);
        writer.u64(self.head_sequence);
        write_ids(&mut writer, &self.stores)?;
        Ok(writer.finish())
    }

    fn check_shape(&self) -> Result<(), Error> {
        check_ids(&self.stores, "an admission snapshot")
    }
}

fn write_ids(writer: &mut Writer, ids: &[[u8; 32]]) -> Result<(), Error> {
    let count =
        u16::try_from(ids.len()).map_err(|_| Error::BadIndex("an index lists at most 4096 ids"))?;
    writer.u16(count);
    for id in ids {
        writer.bytes(id);
    }
    Ok(())
}

fn read_ids(reader: &mut Reader<'_>) -> Result<Vec<[u8; 32]>, Error> {
    let count = reader.u16()? as usize;
    if count > MAX_INDEX_ENTRIES {
        return Err(Error::BadIndex("an index lists at most 4096 ids"));
    }
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(reader.array()?);
    }
    Ok(ids)
}

fn check_ids(ids: &[[u8; 32]], what: &'static str) -> Result<(), Error> {
    if ids.len() > MAX_INDEX_ENTRIES {
        return Err(Error::BadIndex("an index lists at most 4096 ids"));
    }
    if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::BadIndex(if what.starts_with("a catalog") {
            "catalog index hashes are not sorted and unique"
        } else {
            "admission snapshot store ids are not sorted and unique"
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{self, secret_from_label};
    use crate::Error;

    fn hashes(n: usize) -> Vec<[u8; 32]> {
        (0..n)
            .map(|i| {
                let mut hash = [0u8; 32];
                hash[0] = (i % 251) as u8;
                hash[1] = (i / 251) as u8;
                hash[31] = 1;
                hash
            })
            .collect()
    }

    #[test]
    fn a_catalog_index_round_trips_and_a_bad_signature_or_order_is_dropped() {
        let secret = secret_from_label("catalog-index");
        let mut listings = hashes(3);
        listings.swap(0, 2);
        let index = CatalogIndex::sign(7, 20261123, [4u8; 32], listings, &secret).unwrap();
        assert!(index.listings.windows(2).all(|pair| pair[0] < pair[1]));
        let bytes = index.encode().unwrap();
        let decoded = CatalogIndex::decode(&bytes).unwrap();
        decoded.verify().unwrap();
        assert_eq!(decoded, index);

        let mut flipped = bytes.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xff;
        let bad = CatalogIndex::decode(&flipped).unwrap();
        assert!(bad.verify().is_err());

        let mut unsorted = bytes.clone();
        // The first listing hash starts after magic, version, store id, sequence, day, digest, count.
        let at = 4 + 1 + 32 + 8 + 4 + 32 + 2;
        unsorted.swap(at, at + 32);
        assert!(CatalogIndex::decode(&unsorted).is_err());
        assert!(
            CatalogIndex::sign(1, 20261123, [0u8; 32], vec![[1u8; 32], [1u8; 32]], &secret)
                .is_err()
        );
        assert!(CatalogIndex::sign(
            1,
            20261123,
            [0u8; 32],
            hashes(MAX_INDEX_ENTRIES + 1),
            &secret
        )
        .is_err());
        let mut trailed = bytes;
        trailed.push(0);
        assert!(matches!(
            CatalogIndex::decode(&trailed),
            Err(Error::TrailingBytes)
        ));
    }

    #[test]
    fn an_admission_snapshot_verifies_only_under_the_directory_key() {
        let directory = secret_from_label("admission-index");
        let other = secret_from_label("not-the-directory");
        let mut stores = vec![[2u8; 32], [9u8; 32], [2u8; 32]];
        stores.pop();
        stores.swap(0, 1);
        let index = AdmissionIndex::sign(3, 20261123, 11, stores, &directory).unwrap();
        let bytes = index.encode().unwrap();
        AdmissionIndex::decode(&bytes)
            .unwrap()
            .verify_signer(&sign::xonly(&directory))
            .unwrap();
        assert!(AdmissionIndex::decode(&bytes)
            .unwrap()
            .verify_signer(&sign::xonly(&other))
            .is_err());
        assert!(
            AdmissionIndex::sign(1, 20261123, 1, vec![[1u8; 32], [1u8; 32]], &directory).is_err()
        );
        let mut flipped = bytes;
        flipped[20] ^= 0xff;
        assert!(
            AdmissionIndex::decode(&flipped).is_err()
                || AdmissionIndex::decode(&flipped).unwrap().verify().is_err()
        );
    }
}
