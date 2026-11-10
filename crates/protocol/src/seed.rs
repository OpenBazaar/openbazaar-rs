//! The signed directory head introductions serve.
//!
//! Version 2 is a head: the sequence, the introduction list, and the fee a
//! store pays to be admitted. It has no store list. A store joins with an
//! admission record, not with a new sequence. A later sequence replaces an
//! earlier one. An introduction serves the bytes. It does not sign them.
//!
//! A version 1 decoder rejects the version byte and keeps its old file, so
//! an old node does not blank its catalog. This decoder reads version 2.
//!
//! Wire format, version 2 (integers little-endian, `str` = u16 byte length
//! then UTF-8):
//!
//! ```text
//! body = "OBSD" | 0x02 | signer [32] | sequence u64
//!      | intro_count u16 | onion str * intro_count
//!      | fee key [32] | fee_sats u64 | fee_confs u32 | epoch_blocks u32
//!      | moderator u8 | onion str if moderator is 1
//! doc  = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/seed/v2` of `body`.
//! Onion hosts are sorted, with no repeats. An onion is a v3 host: 56
//! lowercase base32 characters and `.onion`.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::Error;

const TAG: &str = "ob/seed/v2";
const MAGIC: [u8; 4] = *b"OBSD";
const VERSION: u8 = 2;
/// Introduction hosts one head may name. A pass dials three of them.
pub const MAX_INTRODUCTIONS: usize = 64;
/// The fee on the first head the operator signs, in satoshis.
pub const FEE_SATS: u64 = 20_000;
/// Testnet4's default confirmation depth for that fee.
pub const FEE_CONFS: u32 = 1;
/// Blocks a confirmed fee stays inside the market index. Same span as the reputation bond lock.
pub const EPOCH_BLOCKS: u32 = 2_016;
const ONION_HOST_LEN: usize = 56;

/// The fee rules a head names. The document signature stays the shipped key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryTerms {
    pub fee_key: [u8; PUBKEY_LEN],
    pub fee_sats: u64,
    pub fee_confs: u32,
    pub epoch_blocks: u32,
}

impl DirectoryTerms {
    /// 20,000 sats, 1 confirmation, 2,016 blocks, paid to `fee_key`.
    pub fn standard(fee_key: [u8; PUBKEY_LEN]) -> Self {
        Self { fee_key, fee_sats: FEE_SATS, fee_confs: FEE_CONFS, epoch_blocks: EPOCH_BLOCKS }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedDocument {
    pub signer: [u8; PUBKEY_LEN],
    pub sequence: u64,
    pub introductions: Vec<String>,
    pub terms: DirectoryTerms,
    /// Onion of the demo moderator, when the document names one.
    pub moderator: Option<String>,
    pub signature: [u8; SIG_LEN],
}

impl SeedDocument {
    /// Sort the introduction list into the one canonical encoding and sign.
    pub fn sign(
        sequence: u64,
        mut introductions: Vec<String>,
        terms: DirectoryTerms,
        moderator: Option<String>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        introductions.sort();
        let document = Self {
            signer: sign::xonly(secret),
            sequence,
            introductions,
            terms,
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
        let terms = DirectoryTerms {
            fee_key: reader.array()?,
            fee_sats: reader.u64()?,
            fee_confs: reader.u32()?,
            epoch_blocks: reader.u32()?,
        };
        let moderator = match reader.u8()? {
            0 => None,
            1 => Some(reader.str()?),
            _ => return Err(Error::BadSeed("the moderator flag is 0 or 1")),
        };
        let signature = reader.array()?;
        reader.finish()?;
        let document = Self { signer, sequence, introductions, terms, moderator, signature };
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
        writer.bytes(&self.terms.fee_key);
        writer.u64(self.terms.fee_sats);
        writer.u32(self.terms.fee_confs);
        writer.u32(self.terms.epoch_blocks);
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
        if self.introductions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::BadSeed("introductions are not sorted and unique"));
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
            DirectoryTerms::standard(sign::xonly(&secret)),
            Some(onion('c')),
            &secret,
        )
        .unwrap();
        (doc, secret)
    }

    #[test]
    fn a_head_round_trips_and_names_no_stores() {
        let (doc, secret) = sample(4);
        assert_eq!(doc.signer, sign::xonly(&secret));
        assert_eq!(doc.introductions, vec![onion('a'), onion('b')]);
        assert_eq!(doc.terms.fee_sats, FEE_SATS);
        assert_eq!(doc.terms.fee_confs, FEE_CONFS);
        assert_eq!(doc.terms.epoch_blocks, EPOCH_BLOCKS);
        doc.verify().unwrap();
        let bytes = doc.encode().unwrap();
        assert_eq!(SeedDocument::decode(&bytes).unwrap(), doc);
        assert!(!bytes.windows(4).any(|window| window == [0x01, 0x00]), "the body has no store count");
        let mut tampered = bytes.clone();
        tampered[10] ^= 0x01;
        assert!(SeedDocument::decode(&tampered).unwrap().verify().is_err());
        let mut version1 = bytes.clone();
        version1[4] = 1;
        assert!(SeedDocument::decode(&version1).is_err(), "a version 1 document is not a head");
    }

    #[test]
    fn a_different_signer_and_an_unsorted_list_are_refused() {
        let (doc, _) = sample(1);
        let other = sign::xonly(&secret_from_label("other-seed-key"));
        assert!(doc.verify_signer(&other).is_err());
        doc.verify_signer(&doc.signer).unwrap();
        let mut unsorted = doc.clone();
        unsorted.introductions.reverse();
        assert!(unsorted.verify().is_err());
        assert!(SeedDocument::sign(
            1,
            vec!["example.com".into()],
            DirectoryTerms::standard([1u8; 32]),
            None,
            &secret_from_label("seed-document-test"),
        )
        .is_err());
    }
}
