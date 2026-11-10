//! A moderator's signed public profile.
//!
//! The moderator's ruling key signs it, so the profile and the rulings have
//! one identity: the x-only key in `moderator_key`. A name is only a label.
//! `record` is the escrow crate's `ModeratorRecord` bytes, which both parties
//! copy into the escrow terms. This crate does not read them; the escrow
//! crate checks that their signing key is `moderator_key`.

use crate::encode::{Reader, Writer};
use crate::public_text;
use crate::sign::PUBKEY_LEN;
use crate::signed::{Signed, Terms};
use crate::store::MAX_NAME;
use crate::Error;

pub const MAX_MODERATOR_DESCRIPTION: usize = 280;
pub const MAX_MODERATOR_TERMS: usize = 2_000;
pub const MAX_MODERATOR_LANGUAGES: usize = 8;
const MAX_LANGUAGE: usize = 12;
const MAX_ADDRESS: usize = 96;
const MAX_RECORD: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeratorProfile {
    /// The ruling key, x-only. It signs this profile and is the moderator's id.
    pub moderator_key: [u8; PUBKEY_LEN],
    /// Unix seconds. A node keeps the newest profile it has seen for a key.
    pub updated: u64,
    pub name: String,
    pub description: String,
    /// The moderator's dispute policy, in its own words.
    pub terms: String,
    /// Language tags such as `en` or `pt-br`.
    pub languages: Vec<String>,
    /// Where dispute letters are sent and this profile is served.
    pub address: String,
    /// The mailbox key dispute letters are sealed to.
    pub mailbox_key: [u8; 32],
    /// `ModeratorRecord` bytes: the keys, payout, fee and policy in the terms.
    pub record: Vec<u8>,
}

pub type SignedModeratorProfile = Signed<ModeratorProfile>;

impl Terms for ModeratorProfile {
    const TAG: &'static str = "ob/moderator-profile/v1";
    const MAGIC: [u8; 4] = *b"OBMP";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.moderator_key
    }

    fn check(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() || self.name.len() > MAX_NAME {
            return Err(Error::BadTerms("a moderator name is 1 to 64 bytes"));
        }
        if self.description.len() > MAX_MODERATOR_DESCRIPTION {
            return Err(Error::BadTerms("a moderator description is at most 280 bytes"));
        }
        if self.terms.len() > MAX_MODERATOR_TERMS {
            return Err(Error::BadTerms("moderator terms are at most 2000 bytes"));
        }
        if self.languages.len() > MAX_MODERATOR_LANGUAGES {
            return Err(Error::BadTerms("a moderator lists at most 8 languages"));
        }
        let tag = |language: &String| {
            (2..=MAX_LANGUAGE).contains(&language.len())
                && language.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        };
        if !self.languages.iter().all(tag) {
            return Err(Error::BadTerms("a language is a tag such as en or pt-br"));
        }
        if self.address.is_empty() || self.address.len() > MAX_ADDRESS {
            return Err(Error::BadTerms("a moderator address is 1 to 96 bytes"));
        }
        if self.record.is_empty() || self.record.len() > MAX_RECORD {
            return Err(Error::BadTerms("a moderator record is 1 to 512 bytes"));
        }
        Ok(())
    }

    fn check_public_text(&self) -> Result<(), Error> {
        public_text::check("moderator name", &self.name)?;
        public_text::check("moderator description", &self.description)?;
        public_text::check("moderator terms", &self.terms)
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.moderator_key);
        writer.u64(self.updated);
        writer.str(&self.name)?;
        writer.str(&self.description)?;
        writer.str(&self.terms)?;
        writer.u8(u8::try_from(self.languages.len()).map_err(|_| Error::FieldTooLong)?);
        for language in &self.languages {
            writer.str(language)?;
        }
        writer.str(&self.address)?;
        writer.bytes(&self.mailbox_key);
        writer.blob(&self.record)
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let moderator_key = reader.array()?;
        let updated = reader.u64()?;
        let name = reader.str()?;
        let description = reader.str()?;
        let terms = reader.str()?;
        let count = usize::from(reader.u8()?);
        if count > MAX_MODERATOR_LANGUAGES {
            return Err(Error::BadTerms("a moderator lists at most 8 languages"));
        }
        let languages = (0..count).map(|_| reader.str()).collect::<Result<_, _>>()?;
        Ok(Self {
            moderator_key,
            updated,
            name,
            description,
            terms,
            languages,
            address: reader.str()?,
            mailbox_key: reader.array()?,
            record: reader.blob()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign;

    fn profile(label: &str) -> ModeratorProfile {
        ModeratorProfile {
            moderator_key: sign::xonly(&sign::secret_from_label(label)),
            updated: 1_794_000_000,
            name: "Harbor Arbitration".into(),
            description: "Disputes about physical goods.".into(),
            terms: "Both sides send tracking and photos. A ruling is given within three days.".into(),
            languages: vec!["en".into(), "pt-br".into()],
            address: format!("http://{}.onion", "a".repeat(56)),
            mailbox_key: [7; 32],
            record: vec![1, 2, 3],
        }
    }

    #[test]
    fn a_signed_profile_round_trips_and_verifies() {
        let secret = sign::secret_from_label("moderator-profile");
        let signed = Signed::sign(profile("moderator-profile"), &secret).unwrap();
        let decoded = SignedModeratorProfile::decode(&signed.encode().unwrap()).unwrap();
        assert_eq!(decoded, signed);
        decoded.verify().unwrap();
    }

    #[test]
    fn a_changed_or_wrongly_keyed_profile_is_refused() {
        let secret = sign::secret_from_label("moderator-profile");
        let signed = Signed::sign(profile("moderator-profile"), &secret).unwrap();
        for edit in [
            (|p: &mut ModeratorProfile| p.name = "Harbour Arbitration".into()) as fn(&mut ModeratorProfile),
            |p| p.record = vec![1, 2, 4],
            |p| p.address = format!("http://{}.onion", "b".repeat(56)),
            |p| p.mailbox_key = [8; 32],
            |p| p.updated += 1,
            |p| p.moderator_key = sign::xonly(&sign::secret_from_label("someone-else")),
        ] {
            let mut changed = signed.clone();
            edit(&mut changed.terms);
            let bytes = changed.encode().unwrap();
            assert!(SignedModeratorProfile::decode(&bytes).unwrap().verify().is_err());
        }
        // Another key cannot sign for this moderator.
        let other = sign::secret_from_label("someone-else");
        assert!(matches!(Signed::sign(profile("moderator-profile"), &other), Err(Error::BadKey)));
    }

    #[test]
    fn a_profile_with_bad_fields_or_private_text_is_refused() {
        let secret = sign::secret_from_label("moderator-profile");
        let bad = |edit: fn(&mut ModeratorProfile)| {
            let mut terms = profile("moderator-profile");
            edit(&mut terms);
            match Signed::sign(terms, &secret) {
                Ok(signed) => SignedModeratorProfile::decode(&signed.encode().unwrap()).is_err(),
                Err(_) => true,
            }
        };
        assert!(bad(|p| p.name = " ".into()));
        assert!(bad(|p| p.name = "n".repeat(65)));
        assert!(bad(|p| p.languages = vec!["English".into()]));
        assert!(bad(|p| p.languages = vec!["en".into(); 9]));
        assert!(bad(|p| p.address.clear()));
        assert!(bad(|p| p.record.clear()));
        assert!(bad(|p| p.description = "Write to june@example.com".into()));
    }
}
