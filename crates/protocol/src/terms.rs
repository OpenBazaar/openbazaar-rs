//! The fee terms a directory head schedules.
//!
//! A head carries up to [`MAX_TERMS`] entries, sorted by start height. An
//! admission is judged by the entry in force at the height that confirmed its
//! fee, and its epoch runs from that height. A later head may drop entries at
//! the front once nothing paid under them can still be listed, and may add
//! entries at the end. It may not change an entry a node already holds.
//!
//! Each added entry names the height it was announced at and starts at least
//! [`notice_blocks`] after it. A node that saw the head current at height `L`
//! refuses an added entry announced before `L - SKEW_BLOCKS`: that entry was
//! signed earlier than its announcement says, or kept back from this node.

use bitcoin::secp256k1::XOnlyPublicKey;

use crate::encode::{Reader, Writer};
use crate::sign::PUBKEY_LEN;
use crate::Error;

/// The fee on the first head the operator signs, in satoshis.
pub const FEE_SATS: u64 = 20_000;
/// Testnet4's default confirmation depth for that fee.
pub const FEE_CONFS: u32 = 1;
/// Blocks a confirmed fee stays inside the market index. Same span as the reputation bond lock.
pub const EPOCH_BLOCKS: u32 = 2_016;
/// Entries one head may schedule.
pub const MAX_TERMS: usize = 8;
/// About a year. Bounds chain scans and how long an entry must be kept.
pub const MAX_EPOCH_BLOCKS: u32 = 52_416;
/// The P2TR dust limit. A smaller fee output is not standard.
pub const MIN_FEE_SATS: u64 = 330;
/// Notice for an entry that asks more of a store than the one before it.
pub const NOTICE_BLOCKS: u32 = 1_008;
/// Notice for an entry that only asks less.
pub const RELAX_NOTICE_BLOCKS: u32 = 288;
/// How late a node may see a head and still accept its new entries.
pub const SKEW_BLOCKS: u32 = 144;
const _: () = assert!(NOTICE_BLOCKS > SKEW_BLOCKS && RELAX_NOTICE_BLOCKS > SKEW_BLOCKS);
/// Encoded size of one entry.
pub const TERMS_ENTRY_LEN: usize = 4 + 4 + 32 + 8 + 4 + 4;

/// The fee rules one entry names. The document signature stays the shipped key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryTerms {
    pub fee_key: [u8; PUBKEY_LEN],
    pub fee_sats: u64,
    pub fee_confs: u32,
    pub epoch_blocks: u32,
}

impl DirectoryTerms {
    /// 20,000 sats, 1 confirmation, 2,016 blocks, paid to each store's key
    /// tweaked from `fee_key`.
    pub fn standard(fee_key: [u8; PUBKEY_LEN]) -> Self {
        Self { fee_key, fee_sats: FEE_SATS, fee_confs: FEE_CONFS, epoch_blocks: EPOCH_BLOCKS }
    }

    /// Whether `next` asks more of a store than `self`: a new key, a higher
    /// fee, more confirmations, or a shorter epoch.
    pub fn tightened_by(&self, next: &DirectoryTerms) -> bool {
        next.fee_key != self.fee_key
            || next.fee_sats > self.fee_sats
            || next.fee_confs > self.fee_confs
            || next.epoch_blocks < self.epoch_blocks
    }

    fn check(&self) -> Result<(), Error> {
        if XOnlyPublicKey::from_slice(&self.fee_key).is_err() {
            return Err(Error::BadSeed("a fee key is not a public key"));
        }
        if self.fee_sats < MIN_FEE_SATS {
            return Err(Error::BadSeed("a fee is below the dust limit"));
        }
        if self.fee_confs == 0 || self.fee_confs > self.epoch_blocks || self.epoch_blocks > MAX_EPOCH_BLOCKS {
            return Err(Error::BadSeed("confirmations and epoch are out of range"));
        }
        Ok(())
    }
}

/// Blocks between announcing `next` and its start, after `prev`.
pub fn notice_blocks(prev: &DirectoryTerms, next: &DirectoryTerms) -> u32 {
    if prev.tightened_by(next) {
        NOTICE_BLOCKS
    } else {
        RELAX_NOTICE_BLOCKS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermsEntry {
    /// First block height whose fee payments these terms judge.
    pub start_height: u32,
    /// The operator's chain tip when it signed this entry.
    pub announced_height: u32,
    pub terms: DirectoryTerms,
}

impl TermsEntry {
    /// The first entry of a schedule: in force from height 0.
    pub fn genesis(terms: DirectoryTerms) -> Self {
        Self { start_height: 0, announced_height: 0, terms }
    }

    /// One past the last height at which an admission paid under this entry
    /// can still be listed, when `next` starts after it.
    pub fn retired_at(&self, next: &TermsEntry) -> u32 {
        next.start_height.saturating_add(self.terms.epoch_blocks)
    }

    pub(crate) fn write(&self, writer: &mut Writer) {
        writer.u32(self.start_height);
        writer.u32(self.announced_height);
        writer.bytes(&self.terms.fee_key);
        writer.u64(self.terms.fee_sats);
        writer.u32(self.terms.fee_confs);
        writer.u32(self.terms.epoch_blocks);
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            start_height: reader.u32()?,
            announced_height: reader.u32()?,
            terms: DirectoryTerms {
                fee_key: reader.array()?,
                fee_sats: reader.u64()?,
                fee_confs: reader.u32()?,
                epoch_blocks: reader.u32()?,
            },
        })
    }
}

/// The node's view when it weighs a new head against the one it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadView {
    /// The chain tip now, when the chain answered.
    pub tip: Option<u32>,
    /// The tip at the last pass where this node held the newest head offered. 0 before the first.
    pub last_current: u32,
}

/// Why a new head does not replace the held one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It changes terms this node holds, or adds terms announced too early. It stays refused.
    Rewrites(&'static str),
    /// It may pass once the chain tip moves on.
    NotYet(&'static str),
}

impl Refusal {
    pub fn reason(&self) -> &'static str {
        match self {
            Refusal::Rewrites(reason) | Refusal::NotYet(reason) => reason,
        }
    }
}

/// One to [`MAX_TERMS`] entries, sorted by start height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermsSchedule {
    entries: Vec<TermsEntry>,
}

impl From<DirectoryTerms> for TermsSchedule {
    fn from(terms: DirectoryTerms) -> Self {
        Self::single(terms)
    }
}

impl TermsSchedule {
    pub fn new(entries: Vec<TermsEntry>) -> Result<Self, Error> {
        let schedule = Self { entries };
        schedule.check()?;
        Ok(schedule)
    }

    /// One entry in force from height 0: a version 2 head, and the first version 3 head.
    pub fn single(terms: DirectoryTerms) -> Self {
        Self { entries: vec![TermsEntry::genesis(terms)] }
    }

    pub fn entries(&self) -> &[TermsEntry] {
        &self.entries
    }

    /// Whether this is one entry in force from height 0, which version 2 can carry.
    pub fn is_single(&self) -> bool {
        matches!(self.entries.as_slice(), [only] if only.start_height == 0 && only.announced_height == 0)
    }

    /// The entry that judges a fee confirmed at `height`.
    pub fn at(&self, height: u32) -> Option<&TermsEntry> {
        self.entries.iter().rev().find(|entry| entry.start_height <= height)
    }

    /// The first entry that starts after `height`.
    pub fn after(&self, height: u32) -> Option<&TermsEntry> {
        self.entries.iter().find(|entry| entry.start_height > height)
    }

    /// Leading entries no admission can still be listed under at `tip`.
    pub fn prunable(&self, tip: u32) -> usize {
        self.entries.windows(2).take_while(|pair| tip >= pair[0].retired_at(&pair[1])).count()
    }

    /// The largest epoch any entry names.
    pub fn max_epoch(&self) -> u32 {
        self.entries.iter().map(|entry| entry.terms.epoch_blocks).max().unwrap_or(0)
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.entries.is_empty() || self.entries.len() > MAX_TERMS {
            return Err(Error::BadSeed("a head schedules one to eight fee terms"));
        }
        for entry in &self.entries {
            entry.terms.check()?;
            if entry.announced_height > entry.start_height {
                return Err(Error::BadSeed("fee terms start before they were announced"));
            }
        }
        for pair in self.entries.windows(2) {
            let (prev, next) = (&pair[0], &pair[1]);
            if next.start_height <= prev.start_height || next.announced_height < prev.announced_height {
                return Err(Error::BadSeed("fee terms are not in start order"));
            }
            if next.start_height - next.announced_height < notice_blocks(&prev.terms, &next.terms) {
                return Err(Error::BadSeed("fee terms start without enough notice"));
            }
        }
        Ok(())
    }

    /// Whether `next` may replace this schedule. `next` is this schedule with
    /// a prefix dropped and entries added at the end. A dropped entry must be
    /// retired at the tip. An added entry must not be announced before this
    /// node last saw the head current, less [`SKEW_BLOCKS`].
    pub fn successor(&self, next: &TermsSchedule, view: HeadView) -> Result<(), Refusal> {
        let (held, next) = (&self.entries, &next.entries);
        if held == next {
            return Ok(());
        }
        let (Some(first), Some(last)) = (next.first(), held.last()) else {
            return Err(Refusal::Rewrites("a head has no fee terms"));
        };
        let dropped = match held.iter().position(|entry| entry == first) {
            Some(index) => index,
            None if first.start_height > last.start_height => held.len(),
            None => return Err(Refusal::Rewrites("the head changes fee terms this node holds")),
        };
        let kept = held.len() - dropped;
        if next.len() < kept || next[..kept] != held[dropped..] {
            return Err(Refusal::Rewrites("the head changes fee terms this node holds"));
        }
        let added = &next[kept..];
        if let Some(first_added) = added.first() {
            if first_added.start_height <= last.start_height {
                return Err(Refusal::Rewrites("the head changes fee terms this node holds"));
            }
            if first_added.start_height - first_added.announced_height < notice_blocks(&last.terms, &first_added.terms)
                || first_added.announced_height < last.announced_height
            {
                return Err(Refusal::Rewrites("new fee terms start without enough notice"));
            }
        }
        for entry in added {
            check_added(entry, view)?;
        }
        if dropped > 0 {
            let Some(tip) = view.tip else { return Err(Refusal::NotYet("the chain tip is unknown")) };
            let chain: Vec<&TermsEntry> = held.iter().chain(added.iter()).collect();
            if (0..dropped).any(|index| tip < chain[index].retired_at(chain[index + 1])) {
                return Err(Refusal::NotYet("dropped fee terms may still list a store"));
            }
        }
        Ok(())
    }
}

/// The check every entry a node has not held passes.
fn check_added(entry: &TermsEntry, view: HeadView) -> Result<(), Refusal> {
    if entry.announced_height < view.last_current.saturating_sub(SKEW_BLOCKS) {
        return Err(Refusal::Rewrites("new fee terms were announced before this node last saw the head current"));
    }
    Ok(())
}
