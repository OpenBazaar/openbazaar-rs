//! Signed label sets from third-party labelers.
//!
//! A labeler is a BIP-340 key. It publishes numbered label sets; each set
//! is signed by that key and addressed by the hash of its body, like a
//! listing. A set never changes a listing or a store. A buyer's client
//! follows labelers it chose, and uses their labels only to hide or warn on
//! that device.
//!
//! Wire format, version 1 (integers little-endian, `str` = u16 byte length
//! then UTF-8):
//!
//! ```text
//! body  = "OBLB" | 0x01 | labeler [32] | name str | sequence u64
//!       | issued_on u32 | count u16 | entry * count
//! entry = kind u8 (0 listing, 1 store) | target [32] | flags u8
//!       | rating u8 | tags u16 | reason str
//! set   = body | signature [64]
//! ```
//!
//! The signature is BIP-340 over the tagged hash `ob/labels/v1` of `body`;
//! the set's address is SHA-256 of `body`. Flag bit 0 retracts the
//! labeler's earlier label on that target; a retraction carries rating 0,
//! tags 0, and no reason. Other flag bits must be zero. Entries are sorted
//! by (kind, target) with no repeats, so one set has one encoding. A reason
//! is a short code (`[a-z0-9-]`, at most 32 bytes), not free text.
//!
//! For each labeler and target, the entry in the highest-numbered set that
//! names the target is the current one, so a later set may carry only
//! changes.

use std::collections::BTreeMap;

use bitcoin::secp256k1::SecretKey;

use crate::content::{ContentLabel, ContentRating, ContentTags};
use crate::encode::{self, Reader, Writer};
use crate::listing::ListingHash;
use crate::sign::{self, PUBKEY_LEN, SIG_LEN};
use crate::store::StoreId;
use crate::Error;

const TAG: &str = "ob/labels/v1";
const MAGIC: [u8; 4] = *b"OBLB";
const VERSION: u8 = 1;
const RETRACT: u8 = 0x01;
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_NAME: usize = 64;
pub const MAX_REASON: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelTarget {
    Listing(ListingHash),
    Store(StoreId),
}

impl LabelTarget {
    fn kind(self) -> u8 {
        match self {
            LabelTarget::Listing(_) => 0,
            LabelTarget::Store(_) => 1,
        }
    }

    fn bytes(&self) -> &[u8; 32] {
        match self {
            LabelTarget::Listing(hash) => hash.as_bytes(),
            LabelTarget::Store(id) => id.as_bytes(),
        }
    }

    pub fn kind_str(self) -> &'static str {
        match self {
            LabelTarget::Listing(_) => "listing",
            LabelTarget::Store(_) => "store",
        }
    }

    pub fn to_hex(self) -> String {
        encode::to_hex(self.bytes())
    }

    fn sort_key(&self) -> (u8, [u8; 32]) {
        (self.kind(), *self.bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelEntry {
    pub target: LabelTarget,
    /// `None` retracts this labeler's earlier label on `target`.
    pub label: Option<ContentLabel>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelSet {
    pub labeler: [u8; PUBKEY_LEN],
    pub name: String,
    pub sequence: u64,
    pub issued_on: u32,
    pub entries: Vec<LabelEntry>,
    pub signature: [u8; SIG_LEN],
}

impl LabelSet {
    /// Sorts `entries` into canonical order. Two entries for one target are refused.
    pub fn sign(
        name: impl Into<String>,
        sequence: u64,
        issued_on: u32,
        mut entries: Vec<LabelEntry>,
        secret: &SecretKey,
    ) -> Result<Self, Error> {
        entries.sort_by_key(|entry| entry.target.sort_key());
        let set = Self {
            labeler: sign::xonly(secret),
            name: name.into(),
            sequence,
            issued_on,
            entries,
            signature: [0u8; SIG_LEN],
        };
        let body = set.encode_body()?;
        let signature = sign::sign(TAG, &body, secret);
        Ok(Self { signature, ..set })
    }

    pub fn verify(&self) -> Result<(), Error> {
        let body = self.encode_body()?;
        sign::verify(TAG, &body, &self.labeler, &self.signature)
    }

    pub fn content_hash(&self) -> Result<[u8; 32], Error> {
        Ok(sign::content_hash(&self.encode_body()?))
    }

    pub fn labeler_hex(&self) -> String {
        encode::to_hex(&self.labeler)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = self.encode_body()?;
        bytes.extend_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (set, len) = Self::decode_one(bytes)?;
        if len != bytes.len() {
            return Err(Error::TrailingBytes);
        }
        Ok(set)
    }

    /// Decode back-to-back sets, as a labeler URL serves them.
    pub fn decode_all(mut bytes: &[u8]) -> Result<Vec<Self>, Error> {
        let mut sets = Vec::new();
        while !bytes.is_empty() {
            let (set, len) = Self::decode_one(bytes)?;
            sets.push(set);
            bytes = &bytes[len..];
        }
        Ok(sets)
    }

    fn decode_one(bytes: &[u8]) -> Result<(Self, usize), Error> {
        let mut reader = Reader::new(bytes);
        reader.expect_magic(&MAGIC, VERSION)?;
        let labeler = reader.array()?;
        let name = reader.str()?;
        let sequence = reader.u64()?;
        let issued_on = reader.u32()?;
        let count = reader.u16()? as usize;
        if count > MAX_ENTRIES {
            return Err(Error::BadLabel);
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(decode_entry(&mut reader)?);
        }
        let signature = reader.array()?;
        let set = Self {
            labeler,
            name,
            sequence,
            issued_on,
            entries,
            signature,
        };
        set.check()?;
        Ok((set, reader.position()))
    }

    fn check(&self) -> Result<(), Error> {
        if self.name.len() > MAX_NAME || self.entries.len() > MAX_ENTRIES {
            return Err(Error::FieldTooLong);
        }
        for pair in self.entries.windows(2) {
            if pair[0].target.sort_key() >= pair[1].target.sort_key() {
                return Err(Error::BadLabel);
            }
        }
        for entry in &self.entries {
            let reason_ok = entry.reason.len() <= MAX_REASON
                && entry
                    .reason
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
            if !reason_ok || (entry.label.is_none() && !entry.reason.is_empty()) {
                return Err(Error::BadLabel);
            }
        }
        Ok(())
    }

    fn encode_body(&self) -> Result<Vec<u8>, Error> {
        self.check()?;
        let mut writer = Writer::new();
        writer.bytes(&MAGIC);
        writer.u8(VERSION);
        writer.bytes(&self.labeler);
        writer.str(&self.name)?;
        writer.u64(self.sequence);
        writer.u32(self.issued_on);
        writer.u16(self.entries.len() as u16);
        for entry in &self.entries {
            writer.u8(entry.target.kind());
            writer.bytes(entry.target.bytes());
            let label = entry.label.unwrap_or(ContentLabel::GENERAL);
            writer.u8(if entry.label.is_none() { RETRACT } else { 0 });
            writer.u8(label.rating.tag());
            writer.u16(label.tags.bits());
            writer.str(&entry.reason)?;
        }
        Ok(writer.finish())
    }
}

fn decode_entry(reader: &mut Reader<'_>) -> Result<LabelEntry, Error> {
    let kind = reader.u8()?;
    let bytes: [u8; 32] = reader.array()?;
    let target = match kind {
        0 => LabelTarget::Listing(ListingHash::from_bytes(bytes)),
        1 => LabelTarget::Store(StoreId::from_bytes(bytes)),
        _ => return Err(Error::BadLabel),
    };
    let flags = reader.u8()?;
    let rating = reader.u8()?;
    let tags = reader.u16()?;
    let reason = reader.str()?;
    let label = match flags {
        0 => Some(ContentLabel::new(ContentRating::from_tag(rating)?, ContentTags::from_bits(tags))),
        RETRACT if rating == 0 && tags == 0 => None,
        _ => return Err(Error::BadLabel),
    };
    Ok(LabelEntry { target, label, reason })
}

/// One labeler's current label on one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLabel {
    pub labeler: [u8; PUBKEY_LEN],
    pub labeler_name: String,
    pub target: LabelTarget,
    pub label: ContentLabel,
    pub reason: String,
}

/// Current labels from verified sets: per labeler and target, the entry
/// from the highest sequence wins, and a retraction leaves no label.
///
/// The caller verifies each set first.
pub fn resolve_labels(sets: &[LabelSet]) -> Vec<ResolvedLabel> {
    let mut ordered: Vec<&LabelSet> = sets.iter().collect();
    ordered.sort_by_key(|set| (set.labeler, set.sequence));
    let mut current: BTreeMap<([u8; 32], (u8, [u8; 32])), (&LabelSet, &LabelEntry)> = BTreeMap::new();
    let mut names: BTreeMap<[u8; 32], &str> = BTreeMap::new();
    for set in ordered {
        names.insert(set.labeler, &set.name);
        for entry in &set.entries {
            current.insert((set.labeler, entry.target.sort_key()), (set, entry));
        }
    }
    current
        .into_values()
        .filter_map(|(set, entry)| {
            Some(ResolvedLabel {
                labeler: set.labeler,
                labeler_name: names[&set.labeler].to_string(),
                target: entry.target,
                label: entry.label?,
                reason: entry.reason.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> SecretKey {
        sign::secret_from_label("labeler-test")
    }

    fn listing(byte: u8) -> LabelTarget {
        LabelTarget::Listing(ListingHash::from_bytes([byte; 32]))
    }

    fn entry(target: LabelTarget, rating: ContentRating, tags: ContentTags, reason: &str) -> LabelEntry {
        LabelEntry {
            target,
            label: Some(ContentLabel::new(rating, tags)),
            reason: reason.to_string(),
        }
    }

    fn retract(target: LabelTarget) -> LabelEntry {
        LabelEntry { target, label: None, reason: String::new() }
    }

    fn sample() -> LabelSet {
        LabelSet::sign(
            "Test labels",
            1,
            20261106,
            vec![
                entry(LabelTarget::Store(StoreId::from_bytes([9; 32])), ContentRating::Adult, ContentTags::empty(), "adult-store"),
                entry(listing(2), ContentRating::Sensitive, ContentTags::WEAPONS, "blade"),
                retract(listing(1)),
            ],
            &secret(),
        )
        .unwrap()
    }

    /// Byte offset of entry `index`'s kind byte in an encoded set.
    fn entry_offset(set: &LabelSet, index: usize) -> usize {
        let mut offset = 4 + 1 + 32 + 2 + set.name.len() + 8 + 4 + 2;
        for entry in &set.entries[..index] {
            offset += 1 + 32 + 1 + 1 + 2 + 2 + entry.reason.len();
        }
        offset
    }

    #[test]
    fn round_trip_is_canonical_and_verifies() {
        let set = sample();
        let bytes = set.encode().unwrap();
        let decoded = LabelSet::decode(&bytes).unwrap();
        assert_eq!(decoded, set);
        decoded.verify().unwrap();
        assert_eq!(decoded.encode().unwrap(), bytes);
        assert_eq!(set.entries[0].target, listing(1), "listings sort before stores");
        assert_eq!(set.labeler, sign::xonly(&secret()));
        assert_eq!(set.content_hash().unwrap(), sign::content_hash(&bytes[..bytes.len() - SIG_LEN]));

        let mut two = bytes.clone();
        two.extend_from_slice(&bytes);
        assert_eq!(LabelSet::decode_all(&two).unwrap(), vec![set.clone(), set]);
    }

    #[test]
    fn edits_to_any_signed_field_fail_verify() {
        let set = sample();
        let mut edits: Vec<LabelSet> = Vec::new();
        let mut softened = set.clone();
        softened.entries[1].label = Some(ContentLabel::GENERAL);
        edits.push(softened);
        let mut untagged = set.clone();
        untagged.entries[1].label = Some(ContentLabel::new(ContentRating::Sensitive, ContentTags::empty()));
        edits.push(untagged);
        let mut unretracted = set.clone();
        unretracted.entries[0].label = Some(ContentLabel::GENERAL);
        edits.push(unretracted);
        let mut retargeted = set.clone();
        retargeted.entries[1].target = listing(3);
        edits.push(retargeted);
        let mut renumbered = set.clone();
        renumbered.sequence += 1;
        edits.push(renumbered);
        let mut renamed = set.clone();
        renamed.name.push('!');
        edits.push(renamed);
        let mut dropped = set.clone();
        dropped.entries.remove(1);
        edits.push(dropped);
        let mut reasoned = set.clone();
        reasoned.entries[1].reason = "toy".to_string();
        edits.push(reasoned);
        for edit in edits {
            assert_eq!(edit.verify(), Err(Error::BadSignature), "{edit:?}");
        }
    }

    #[test]
    fn a_set_is_bound_to_its_labeler_key() {
        let set = sample();
        let mut swapped = set.clone();
        swapped.labeler = sign::xonly(&sign::secret_from_label("someone-else"));
        assert_eq!(swapped.verify(), Err(Error::BadSignature));

        let mut bad_sig = set;
        bad_sig.signature[10] ^= 0x01;
        assert_eq!(bad_sig.verify(), Err(Error::BadSignature));
    }

    #[test]
    fn relay_byte_flips_fail_decode_or_verify() {
        let set = sample();
        let bytes = set.encode().unwrap();
        let start = entry_offset(&set, 1);
        for offset in [start + 33, start + 34, start + 35] {
            let mut edited = bytes.clone();
            edited[offset] ^= 0x01;
            let result = LabelSet::decode(&edited).and_then(|set| set.verify());
            assert!(result.is_err(), "flip at {offset}");
        }
    }

    #[test]
    fn non_canonical_sets_do_not_decode() {
        let set = sample();
        let bytes = set.encode().unwrap();
        let first = entry_offset(&set, 0);

        let mut unknown_kind = bytes.clone();
        unknown_kind[first] = 7;
        assert_eq!(LabelSet::decode(&unknown_kind), Err(Error::BadLabel));

        let mut unknown_flag = bytes.clone();
        unknown_flag[first + 33] = 0x02;
        assert_eq!(LabelSet::decode(&unknown_flag), Err(Error::BadLabel));

        let mut rated_retraction = bytes.clone();
        rated_retraction[first + 34] = 2;
        assert_eq!(LabelSet::decode(&rated_retraction), Err(Error::BadLabel));

        let mut unknown_rating = bytes.clone();
        unknown_rating[entry_offset(&set, 1) + 34] = 3;
        assert_eq!(LabelSet::decode(&unknown_rating), Err(Error::UnknownRating));

        let mut unsorted = set.clone();
        unsorted.entries.swap(0, 1);
        assert_eq!(unsorted.encode(), Err(Error::BadLabel));
        let mut duplicate = set.clone();
        duplicate.entries[1].target = duplicate.entries[0].target;
        assert_eq!(duplicate.encode(), Err(Error::BadLabel));
        let duplicate_sign = LabelSet::sign("x", 1, 1, vec![retract(listing(1)), retract(listing(1))], &secret());
        assert_eq!(duplicate_sign, Err(Error::BadLabel));
        for reason in ["Has Spaces", "x".repeat(MAX_REASON + 1).as_str()] {
            let signed = LabelSet::sign("x", 1, 1, vec![entry(listing(1), ContentRating::Adult, ContentTags::empty(), reason)], &secret());
            assert_eq!(signed, Err(Error::BadLabel), "{reason}");
        }

        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(LabelSet::decode(&trailing), Err(Error::TrailingBytes));
    }

    #[test]
    fn highest_sequence_wins_and_retraction_clears() {
        let first = sample();
        let later = LabelSet::sign(
            "Renamed labels",
            2,
            20261107,
            vec![
                retract(listing(2)),
                entry(listing(1), ContentRating::Adult, ContentTags::NUDITY, "explicit"),
            ],
            &secret(),
        )
        .unwrap();
        let other = LabelSet::sign("Other", 1, 1, vec![entry(listing(2), ContentRating::Sensitive, ContentTags::empty(), "")], &sign::secret_from_label("other")).unwrap();

        for sets in [vec![first.clone(), later.clone(), other.clone()], vec![other.clone(), later.clone(), first.clone()]] {
            let labels = resolve_labels(&sets);
            let mine: Vec<_> = labels.iter().filter(|label| label.labeler == first.labeler).collect();
            assert_eq!(mine.len(), 2, "{mine:?}");
            assert_eq!((mine[0].target, mine[0].label.rating, mine[0].reason.as_str()), (listing(1), ContentRating::Adult, "explicit"));
            assert!(matches!(mine[1].target, LabelTarget::Store(_)));
            assert!(mine.iter().all(|label| label.labeler_name == "Renamed labels"));
            assert!(labels.iter().any(|label| label.labeler == other.labeler && label.target == listing(2)));
        }

        let older_only = resolve_labels(&[first]);
        assert!(older_only.iter().any(|label| label.target == listing(2)));
    }
}
