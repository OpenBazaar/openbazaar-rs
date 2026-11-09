//! The signed seed document introductions serve.
//!
//! A peer trusts the document for availability only. The signature proves
//! which key published this sequence, the introduction list, and the store
//! keys a node may replicate. A later sequence replaces an earlier one. An
//! introduction that withholds the document, or serves an older one, cannot
//! forge a store.
//!
//! Wire format, version 1 (integers little-endian, `str` = u16 byte length
//! then UTF-8):
//!
//! ```text
//! body = "OBSD" | 0x01 | signer [32] | sequence u64
//!      | intro_count u16 | onion str * intro_count
//!      | store_count u16 | store_id [32] * store_count
//!      | moderator u8 | onion str if moderator is 1
//! doc  = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/seed/v1` of `body`.
//! Onions and store ids are sorted, with no repeats, so one document has
//! one encoding. An onion is a v3 host: 56 lowercase base32 characters and
//! `.onion`.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/seed/v1";
const MAGIC: [u8; 4] = *b"OBSD";
const VERSION: u8 = 1;
const MAX_INTRODUCTIONS: usize = 8;
const MAX_STORES: usize = 256;
const ONION_HOST_LEN: usize = 56;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedDocument {
    pub signer: [u8; PUBKEY_LEN],
    pub sequence: u64,
    pub introductions: Vec<String>,
    pub stores: Vec<[u8; PUBKEY_LEN]>,
    /// Onion of the demo moderator, when the document names one.
    pub moderator: Option<String>,
    pub signature: [u8; SIG_LEN],
}

impl SeedDocument {
    /// Sort the lists into the one canonical encoding and sign.
    pub fn sign(
        sequence: u64,
        mut introductions: Vec<String>,
        mut stores: Vec<[u8; PUBKEY_LEN]>,
        moderator: Option<String>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        introductions.sort();
        stores.sort();
        let document = Self {
            signer: sign::xonly(secret),
            sequence,
            introductions,
            stores,
            moderator,
            signature: [0u8; SIG_LEN],
        };
        document.check_shape()?;
        let body = document.encode_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self { signature, ..document })
    }

    pub fn verify(&self) -> Result<(), Error> {
        self.check_shape()?;
        let body = self.encode_body()?;
        sign::verify(TAG, &body, &self.signer, &self.signature)
    }

    /// Verify, and require the signer to be `expected`.
    pub fn verify_signer(&self, expected: &[u8; PUBKEY_LEN]) -> Result<(), Error> {
        if self.signer != *expected {
            return Err(Error::BadSignature);
        }
        self.verify()
    }

    pub fn allows(&self, store: &[u8; PUBKEY_LEN]) -> bool {
        self.stores.binary_search(store).is_ok()
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = self.encode_body()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(&MAGIC, VERSION)?;
        let signer = reader.array()?;
        let sequence = reader.u64()?;
        let intro_count = reader.u16()? as usize;
        if intro_count > MAX_INTRODUCTIONS {
            return Err(Error::BadSeed("too many introductions"));
        }
        let mut introductions = Vec::with_capacity(intro_count);
        for _ in 0..intro_count {
            introductions.push(reader.str()?);
        }
        let store_count = reader.u16()? as usize;
        if store_count > MAX_STORES {
            return Err(Error::BadSeed("too many stores"));
        }
        let mut stores = Vec::with_capacity(store_count);
        for _ in 0..store_count {
            stores.push(reader.array()?);
        }
        let moderator = match reader.u8()? {
            0 => None,
            1 => Some(reader.str()?),
            _ => return Err(Error::BadSeed("the moderator flag is 0 or 1")),
        };
        let signature = reader.array()?;
        reader.finish()?;
        let document = Self { signer, sequence, introductions, stores, moderator, signature };
        document.check_shape()?;
        Ok(document)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.signer);
        writer.u64(self.sequence);
        let intro_count = u16::try_from(self.introductions.len()).map_err(|_| Error::BadSeed("too many introductions"))?;
        writer.u16(intro_count);
        for onion in &self.introductions {
            writer.str(onion)?;
        }
        let store_count = u16::try_from(self.stores.len()).map_err(|_| Error::BadSeed("too many stores"))?;
        writer.u16(store_count);
        for store in &self.stores {
            writer.bytes(store);
        }
        match &self.moderator {
            None => writer.u8(0),
            Some(onion) => {
                writer.u8(1);
                writer.str(onion)?;
            }
        }
        Ok(writer.finish())
    }

    fn check_shape(&self) -> Result<(), Error> {
        if self.introductions.len() > MAX_INTRODUCTIONS {
            return Err(Error::BadSeed("too many introductions"));
        }
        if self.stores.len() > MAX_STORES {
            return Err(Error::BadSeed("too many stores"));
        }
        if self.introductions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::BadSeed("introductions are not sorted and unique"));
        }
        if self.stores.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::BadSeed("store keys are not sorted and unique"));
        }
        for onion in self.introductions.iter().chain(self.moderator.as_ref()) {
            if !is_onion_host(onion) {
                return Err(Error::BadSeed("an introduction is not a v3 onion host"));
            }
        }
        Ok(())
    }
}

fn is_onion_host(text: &str) -> bool {
    text.strip_suffix(".onion").is_some_and(|host| {
        host.len() == ONION_HOST_LEN && host.bytes().all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::secret_from_label;

    fn onion(label: char) -> String {
        format!("{}.onion", label.to_string().repeat(ONION_HOST_LEN))
    }

    fn sample(sequence: u64) -> (SeedDocument, SecretKey) {
        let secret = secret_from_label("seed-document-test");
        let doc = SeedDocument::sign(
            sequence,
            vec![onion('b'), onion('a')],
            vec![[2u8; 32], [1u8; 32]],
            Some(onion('c')),
            &secret,
        )
        .unwrap();
        (doc, secret)
    }

    #[test]
    fn a_document_round_trips_and_a_changed_byte_fails() {
        let (doc, secret) = sample(4);
        assert_eq!(doc.signer, sign::xonly(&secret));
        assert_eq!(doc.introductions, vec![onion('a'), onion('b')]);
        assert_eq!(doc.stores, vec![[1u8; 32], [2u8; 32]]);
        doc.verify().unwrap();
        let bytes = doc.encode().unwrap();
        assert_eq!(SeedDocument::decode(&bytes).unwrap(), doc);
        let mut tampered = bytes.clone();
        tampered[10] ^= 0x01;
        assert!(SeedDocument::decode(&tampered).unwrap().verify().is_err());
    }

    #[test]
    fn a_different_signer_and_an_unsorted_list_are_refused() {
        let (doc, _) = sample(1);
        let other = sign::xonly(&secret_from_label("other-seed-key"));
        assert!(doc.verify_signer(&other).is_err());
        doc.verify_signer(&doc.signer).unwrap();
        let mut unsorted = doc.clone();
        unsorted.stores.reverse();
        assert!(unsorted.verify().is_err());
        assert!(SeedDocument::sign(1, vec!["example.com".into()], vec![], None, &secret_from_label("seed-document-test")).is_err());
    }
}
