//! The signed directory head introductions serve.
//!
//! A head is the sequence, the introduction list, and the fee terms a store
//! pays to be admitted. It has no store list. A store joins with an
//! admission record, not with a new sequence. A later sequence replaces an
//! earlier one when its terms extend the held ones ([`TermsSchedule::successor`]).
//! An introduction serves the bytes. It does not sign them.
//!
//! Version 3 schedules the terms. Version 2 names one set, read here as a
//! schedule of one entry in force from height 0. A version 2 decoder rejects
//! the version byte and keeps its old file, so an old node keeps its terms.
//!
//! Wire format (integers little-endian, `str` = u16 byte length then UTF-8):
//!
//! ```text
//! body  = "OBSD" | version u8 | signer [32] | sequence u64
//!       | intro_count u16 | onion str * intro_count
//!       | terms                                   (by version, below)
//!       | moderator u8 | onion str if moderator is 1
//! v2    = fee key [32] | fee_sats u64 | fee_confs u32 | epoch_blocks u32
//! v3    = terms_count u8 | entry * terms_count   (1 to 8, sorted by start)
//! entry = start_height u32 | announced_height u32
//!       | fee key [32] | fee_sats u64 | fee_confs u32 | epoch_blocks u32
//! doc   = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/seed/v{version}` of
//! `body`. Onion hosts are sorted, with no repeats. An onion is a v3 host: 56
//! lowercase base32 characters and `.onion`.

use bitcoin::secp256k1::SecretKey;

use crate::encode::{Reader, Writer};
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::terms::{DirectoryTerms, TermsEntry, TermsSchedule, MAX_TERMS};
use crate::Error;

const MAGIC: [u8; 4] = *b"OBSD";
const VERSION: u8 = 3;
const VERSION_SINGLE: u8 = 2;
/// Introduction hosts one head may name. A pass dials three of them.
pub const MAX_INTRODUCTIONS: usize = 64;
const ONION_HOST_LEN: usize = 56;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedDocument {
    /// 3, or 2 for a head signed before schedules.
    pub version: u8,
    pub signer: [u8; PUBKEY_LEN],
    pub sequence: u64,
    pub introductions: Vec<String>,
    pub schedule: TermsSchedule,
    /// Onion of the demo moderator, when the document names one.
    pub moderator: Option<String>,
    pub signature: [u8; SIG_LEN],
}

impl SeedDocument {
    /// Sort the introduction list into the one canonical encoding and sign version 3.
    pub fn sign(
        sequence: u64,
        introductions: Vec<String>,
        schedule: impl Into<TermsSchedule>,
        moderator: Option<String>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        Self::sign_version(VERSION, sequence, introductions, schedule.into(), moderator, secret)
    }

    /// A version 2 head, for nodes that do not read schedules.
    pub fn sign_v2(
        sequence: u64,
        introductions: Vec<String>,
        terms: DirectoryTerms,
        moderator: Option<String>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        Self::sign_version(VERSION_SINGLE, sequence, introductions, TermsSchedule::single(terms), moderator, secret)
    }

    fn sign_version(
        version: u8,
        sequence: u64,
        mut introductions: Vec<String>,
        schedule: TermsSchedule,
        moderator: Option<String>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        introductions.sort();
        let document = Self {
            version,
            signer: sign::xonly(secret),
            sequence,
            introductions,
            schedule,
            moderator,
            signature: [0u8; SIG_LEN],
        };
        document.check_shape()?;
        let body = document.encode_body()?;
        let signature = sign::sign(&tag(version), &body, secret);
        Ok(Self { signature, ..document })
    }

    pub fn verify(&self) -> Result<(), Error> {
        self.check_shape()?;
        let body = self.encode_body()?;
        sign::verify(&tag(self.version), &body, &self.signer, &self.signature)
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
        let version = reader.expect_magic_in(&MAGIC, &[VERSION_SINGLE, VERSION])?;
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
        let schedule = if version == VERSION_SINGLE {
            TermsSchedule::single(DirectoryTerms {
                fee_key: reader.array()?,
                fee_sats: reader.u64()?,
                fee_confs: reader.u32()?,
                epoch_blocks: reader.u32()?,
            })
        } else {
            let count = reader.u8()? as usize;
            if count == 0 || count > MAX_TERMS {
                return Err(Error::BadSeed("a head schedules one to eight fee terms"));
            }
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                entries.push(TermsEntry::read(&mut reader)?);
            }
            TermsSchedule::new(entries)?
        };
        let moderator = match reader.u8()? {
            0 => None,
            1 => Some(reader.str()?),
            _ => return Err(Error::BadSeed("the moderator flag is 0 or 1")),
        };
        let signature = reader.array()?;
        reader.finish()?;
        let document = Self { version, signer, sequence, introductions, schedule, moderator, signature };
        document.check_shape()?;
        Ok(document)
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(self.version);
        writer.bytes(&self.signer);
        writer.u64(self.sequence);
        let intro_count = u16::try_from(self.introductions.len()).map_err(|_| Error::BadSeed("too many introductions"))?;
        writer.u16(intro_count);
        for onion in &self.introductions {
            writer.str(onion)?;
        }
        match self.version {
            VERSION_SINGLE => {
                let [only] = self.schedule.entries() else {
                    return Err(Error::BadSeed("a version 2 head names one set of terms"));
                };
                writer.bytes(&only.terms.fee_key);
                writer.u64(only.terms.fee_sats);
                writer.u32(only.terms.fee_confs);
                writer.u32(only.terms.epoch_blocks);
            }
            VERSION => {
                writer.u8(u8::try_from(self.schedule.entries().len()).map_err(|_| Error::BadSeed("too many fee terms"))?);
                for entry in self.schedule.entries() {
                    entry.write(&mut writer);
                }
            }
            _ => return Err(Error::BadVersion),
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
        if self.introductions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::BadSeed("introductions are not sorted and unique"));
        }
        for onion in self.introductions.iter().chain(self.moderator.as_ref()) {
            if !is_onion_host(onion) {
                return Err(Error::BadSeed("an introduction is not a v3 onion host"));
            }
        }
        if self.version == VERSION_SINGLE && !self.schedule.is_single() {
            return Err(Error::BadSeed("a version 2 head names one set of terms"));
        }
        self.schedule.check()
    }
}

fn tag(version: u8) -> String {
    format!("ob/seed/v{version}")
}

fn is_onion_host(text: &str) -> bool {
    text.strip_suffix(".onion").is_some_and(|host| {
        host.len() == ONION_HOST_LEN && host.bytes().all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    })
}

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
