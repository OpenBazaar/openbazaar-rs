//! Short-lived WTB / WTS posts (`OBCL`).
//!
//! A classified is signed by the poster's BIP 340 key (a store key or a
//! buyer-light identity). Directories may index admitted posts; any node
//! can keep its own without a fee. Contact mail is sealed to `mailbox_key`
//! and is not part of this record.

use crate::encode::{Reader, Writer};
use crate::plain_text;
use crate::public_text;
use crate::signed::{check_currency, Signed, Terms};
use crate::sign::PUBKEY_LEN;
use crate::Error;

pub const MAX_TITLE: usize = 120;
pub const MAX_BODY: usize = 4_000;
/// Longest life a signed post may claim, in calendar days.
pub const MAX_LIFE_DAYS: u32 = 30;
/// Suggested default life when the client does not pick one.
pub const DEFAULT_LIFE_DAYS: u32 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassifiedKind {
    Wtb = 1,
    Wts = 2,
}

impl ClassifiedKind {
    fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Wtb),
            2 => Ok(Self::Wts),
            _ => Err(Error::BadTerms("a classified is want-to-buy or want-to-sell")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wtb => "wtb",
            Self::Wts => "wts",
        }
    }

    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "wtb" => Ok(Self::Wtb),
            "wts" => Ok(Self::Wts),
            _ => Err(Error::BadTerms("a classified is want-to-buy or want-to-sell")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassifiedCategory {
    Goods = 1,
    Services = 2,
    Housing = 3,
    Jobs = 4,
    Other = 5,
}

impl ClassifiedCategory {
    fn from_u8(value: u8) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Goods),
            2 => Ok(Self::Services),
            3 => Ok(Self::Housing),
            4 => Ok(Self::Jobs),
            5 => Ok(Self::Other),
            _ => Err(Error::BadTerms("unknown classified category")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goods => "goods",
            Self::Services => "services",
            Self::Housing => "housing",
            Self::Jobs => "jobs",
            Self::Other => "other",
        }
    }

    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "goods" => Ok(Self::Goods),
            "services" => Ok(Self::Services),
            "housing" => Ok(Self::Housing),
            "jobs" => Ok(Self::Jobs),
            "other" => Ok(Self::Other),
            _ => Err(Error::BadTerms("unknown classified category")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classified {
    pub id: [u8; 16],
    pub kind: ClassifiedKind,
    pub category: ClassifiedCategory,
    pub title: String,
    pub body: String,
    /// Coarse region, or empty.
    pub region: String,
    /// Minor units; zero with empty currency means no hint.
    pub price_hint: u64,
    pub currency: String,
    /// YYYYMMDD.
    pub created_on: u32,
    /// YYYYMMDD, at most [`MAX_LIFE_DAYS`] after `created_on`.
    pub expires_on: u32,
    pub poster: [u8; PUBKEY_LEN],
    pub mailbox_key: [u8; 32],
    /// Optional v3 onion hostname (no scheme), or empty.
    pub onion: String,
}

pub type SignedClassified = Signed<Classified>;

impl Classified {
    pub fn expired_on(&self, day: u32) -> bool {
        day > self.expires_on
    }

    pub fn life_days(&self) -> Option<u32> {
        days_between(self.created_on, self.expires_on)
    }
}

impl Terms for Classified {
    const TAG: &'static str = "ob/classified/v1";
    const MAGIC: [u8; 4] = *b"OBCL";
    const VERSION: u8 = 1;

    fn signer(&self) -> [u8; PUBKEY_LEN] {
        self.poster
    }

    fn check(&self) -> Result<(), Error> {
        if self.id == [0u8; 16] {
            return Err(Error::BadTerms("a classified needs a random id"));
        }
        let title = self.title.trim();
        if title.is_empty() || title.len() > MAX_TITLE {
            return Err(Error::BadTerms("a classified title is 1 to 120 bytes"));
        }
        if self.title != title {
            return Err(Error::BadTerms("a classified title has no surrounding spaces"));
        }
        if !plain_text::is_plain(&self.title, false) {
            return Err(Error::BadTerms(plain_text::REFUSAL));
        }
        let body = self.body.trim();
        if body.is_empty() || body.len() > MAX_BODY {
            return Err(Error::BadTerms("a classified body is 1 to 4,000 bytes"));
        }
        if self.body != body {
            return Err(Error::BadTerms("a classified body has no surrounding spaces"));
        }
        if !plain_text::is_plain(&self.body, true) {
            return Err(Error::BadTerms(plain_text::REFUSAL));
        }
        if self.price_hint > 0 {
            check_currency(&self.currency)?;
        } else if !self.currency.is_empty() {
            return Err(Error::BadTerms("a price hint needs an amount"));
        }
        if self.created_on == 0 || self.expires_on == 0 {
            return Err(Error::BadTerms("a classified needs created and expiry days"));
        }
        let Some(life) = days_between(self.created_on, self.expires_on) else {
            return Err(Error::BadTerms("a classified expires on or after the day it is created"));
        };
        if life == 0 || life > MAX_LIFE_DAYS {
            return Err(Error::BadTerms("a classified lives between 1 and 30 days"));
        }
        if self.mailbox_key == [0u8; 32] {
            return Err(Error::BadTerms("a classified needs a mailbox key"));
        }
        if !self.onion.is_empty() && !is_v3_onion(&self.onion) {
            return Err(Error::BadTerms("a classified onion is a v3 hostname"));
        }
        Ok(())
    }

    fn check_public_text(&self) -> Result<(), Error> {
        public_text::check("classified title", &self.title)?;
        public_text::check("classified body", &self.body)?;
        if !self.region.is_empty() {
            public_text::check_region("classified region", &self.region)?;
        }
        Ok(())
    }

    fn write(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.bytes(&self.id);
        writer.u8(self.kind as u8);
        writer.u8(self.category as u8);
        writer.str(&self.title)?;
        writer.str(&self.body)?;
        writer.str(&self.region)?;
        writer.u64(self.price_hint);
        writer.str(&self.currency)?;
        writer.u32(self.created_on);
        writer.u32(self.expires_on);
        writer.bytes(&self.poster);
        writer.bytes(&self.mailbox_key);
        writer.str(&self.onion)?;
        Ok(())
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            id: reader.array()?,
            kind: ClassifiedKind::from_u8(reader.u8()?)?,
            category: ClassifiedCategory::from_u8(reader.u8()?)?,
            title: reader.str()?,
            body: reader.str()?,
            region: reader.str()?,
            price_hint: reader.u64()?,
            currency: reader.str()?,
            created_on: reader.u32()?,
            expires_on: reader.u32()?,
            poster: reader.array()?,
            mailbox_key: reader.array()?,
            onion: reader.str()?,
        })
    }
}

fn is_v3_onion(host: &str) -> bool {
    let host = host.strip_suffix(".onion").unwrap_or(host);
    host.len() == 56 && host.bytes().all(|b| matches!(b, b'a'..=b'z' | b'2'..=b'7'))
}

/// Inclusive calendar span in days for YYYYMMDD values, or `None` if invalid.
fn days_between(start: u32, end: u32) -> Option<u32> {
    let (sy, sm, sd) = ymd(start)?;
    let (ey, em, ed) = ymd(end)?;
    let start = civil_day(sy, sm, sd)?;
    let end = civil_day(ey, em, ed)?;
    end.checked_sub(start)
}

fn ymd(day: u32) -> Option<(i32, u32, u32)> {
    let y = (day / 10_000) as i32;
    let m = (day / 100) % 100;
    let d = day % 100;
    if y < 1970 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

fn civil_day(year: i32, month: u32, day: u32) -> Option<u32> {
    // Days since 1970-01-01, civil calendar (enough for expiry spans).
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u32;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = (era * 146_097) as i64 + doe as i64 - 719_468;
    u32::try_from(days).ok()
}

/// YYYYMMDD for `unix` seconds (UTC).
pub fn day_from_unix(unix: u64) -> u32 {
    let days = (unix / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32) * 10_000 + m * 100 + d
}

/// Add `days` to a YYYYMMDD value.
pub fn add_days(day: u32, days: u32) -> Option<u32> {
    let (y, m, d) = ymd(day)?;
    let start = civil_day(y, m, d)?;
    let end = start.checked_add(days)?;
    Some(day_from_unix(u64::from(end) * 86_400))
}

#[cfg(test)]
mod tests {
    use bitcoin::secp256k1::SecretKey;

    use super::*;
    use crate::sign;

    fn sample(secret: &SecretKey) -> Classified {
        let poster = sign::xonly(secret);
        Classified {
            id: [7u8; 16],
            kind: ClassifiedKind::Wtb,
            category: ClassifiedCategory::Goods,
            title: "Looking for a spare bike chain".into(),
            body: "Prefer a 11-speed chain in good shape. Local pickup.".into(),
            region: "Oregon".into(),
            price_hint: 2_500,
            currency: "USD".into(),
            created_on: 20261125,
            expires_on: 20261202,
            poster,
            mailbox_key: [9u8; 32],
            onion: "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz2345".into(),
        }
    }

    #[test]
    fn a_classified_round_trips_and_rejects_tampering() {
        let secret = SecretKey::from_slice(&[11u8; 32]).unwrap();
        let signed = SignedClassified::sign(sample(&secret), &secret).unwrap();
        signed.verify().unwrap();
        let bytes = signed.encode().unwrap();
        let again = SignedClassified::decode(&bytes).unwrap();
        assert_eq!(again, signed);
        let mut bad = bytes.clone();
        bad[20] ^= 1;
        assert!(SignedClassified::decode(&bad).and_then(|s| s.verify()).is_err());
    }

    #[test]
    fn expiry_and_text_rules_are_enforced() {
        let secret = SecretKey::from_slice(&[11u8; 32]).unwrap();
        let mut post = sample(&secret);
        post.expires_on = 20261125;
        assert!(SignedClassified::sign(post.clone(), &secret).is_err());
        post.expires_on = add_days(20261125, 31).unwrap();
        assert!(SignedClassified::sign(post.clone(), &secret).is_err());
        post = sample(&secret);
        post.title = "Call me at 503 555 0142".into();
        assert!(SignedClassified::sign(post, &secret).is_err());
    }

    #[test]
    fn day_math_matches_calendar_spans() {
        assert_eq!(day_from_unix(0), 19700101);
        assert_eq!(days_between(20261125, 20261202), Some(7));
        assert_eq!(add_days(20261125, 7), Some(20261202));
        assert_eq!(ClassifiedCategory::parse("housing").unwrap().as_str(), "housing");
        assert_eq!(ClassifiedKind::Wts.as_str(), "wts");
        assert_eq!(sample(&SecretKey::from_slice(&[11u8; 32]).unwrap()).onion.len(), 56);
    }
}
