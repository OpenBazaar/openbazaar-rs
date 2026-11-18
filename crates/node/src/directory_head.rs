//! The directory head this node holds, and the rules a new head passes.
//!
//! A higher sequence replaces the held head only when its fee terms extend
//! the held ones ([`TermsSchedule::successor`]). A head that changes terms
//! already held, or adds terms announced before this node last saw the head
//! current, is refused and stays refused: the node keeps its last good terms
//! and says so in `GET /directory/fee`. A node with no head takes the first one
//! that verifies. Once a version 3 head is held, a version 2 head never
//! replaces it. An introduction keeps the highest version 2 head as well and
//! serves it to nodes that do not read version 3.
//!
//! Files in `directory/`: `document.obsd` (the held head), `document-v2.obsd`
//! (the version 2 head served to old nodes), `head-checked` (the tip at the
//! last pass that held the newest head offered), `head-refused` (the last
//! refused sequence and why), and `equivocation-{sequence}.obsd` (a second
//! head signed at a held sequence).
//!
//! [`TermsSchedule::successor`]: openbazaar_protocol::TermsSchedule::successor

use std::fs;
use std::path::{Path, PathBuf};

use openbazaar_protocol::{HeadView, Refusal, SeedDocument};

pub(crate) struct Heads {
    dir: PathBuf,
    document: Option<SeedDocument>,
    legacy: Option<SeedDocument>,
    last_current: u32,
    refused: Option<(u64, String)>,
}

#[derive(Debug, PartialEq, Eq)]
enum Weighed {
    Kept,
    Unchanged,
    Waiting,
    Refused(String),
}

impl Heads {
    /// Read the saved heads. A file that does not verify under `key` is skipped.
    pub(crate) fn load(dir: &Path, key: &[u8; 32]) -> Self {
        let read = |name: &str| -> Option<SeedDocument> {
            let path = dir.join(name);
            let bytes = fs::read(&path).ok()?;
            match SeedDocument::decode(&bytes).map(|document| (document.verify_signer(key), document)) {
                Ok((Ok(()), document)) => Some(document),
                Ok((Err(_), _)) => {
                    eprintln!("ob-node: directory head at {} is not signed by the shipped key", path.display());
                    None
                }
                Err(err) => {
                    eprintln!("ob-node: directory head at {}: {err}", path.display());
                    None
                }
            }
        };
        let document = read("document.obsd");
        if let Some(document) = &document {
            eprintln!("ob-node: directory head sequence {} (version {})", document.sequence, document.version);
        }
        let legacy = read("document-v2.obsd").filter(|legacy| legacy.version == 2);
        let last_current = fs::read_to_string(dir.join("head-checked")).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
        let refused = fs::read_to_string(dir.join("head-refused")).ok().and_then(|text| {
            let (sequence, reason) = text.split_once('\n')?;
            Some((sequence.trim().parse().ok()?, reason.trim().to_string()))
        });
        Self { dir: dir.to_path_buf(), document, legacy, last_current, refused }
    }

    pub(crate) fn document(&self) -> Option<&SeedDocument> {
        self.document.as_ref()
    }

    /// The last head this node refused, with the reason.
    pub(crate) fn refused(&self) -> Option<&(u64, String)> {
        self.refused.as_ref()
    }

    /// Bytes for `QUERY_HEAD`: the held head.
    pub(crate) fn head_bytes(&self) -> Option<Vec<u8>> {
        self.document.as_ref()?.encode().ok()
    }

    /// Bytes for `QUERY_SEED`: a version 2 head while one is kept, else the held head.
    pub(crate) fn seed_bytes(&self) -> Option<Vec<u8>> {
        self.legacy.as_ref().or(self.document.as_ref())?.encode().ok()
    }

    /// Weigh every head one pass fetched, highest sequence first. When a
    /// source answered with version 3 and no head is waiting on the chain, the
    /// tip becomes the last-current height. A version 2 answer cannot carry new
    /// terms, so it does not show this node was offered them.
    pub(crate) fn keep(&mut self, mut fetched: Vec<SeedDocument>, tip: Option<u32>) -> Result<(), String> {
        let answered = fetched.iter().any(|document| document.version >= 3);
        fetched.sort_by(|a, b| b.sequence.cmp(&a.sequence).then(b.version.cmp(&a.version)));
        let mut waiting = false;
        for candidate in fetched {
            waiting |= self.weigh(candidate, tip)? == Weighed::Waiting;
        }
        if let (true, false, Some(tip)) = (answered, waiting, tip) {
            if tip > self.last_current {
                self.last_current = tip;
                atomic(&self.dir.join("head-checked"), format!("{tip}\n").as_bytes())?;
            }
        }
        Ok(())
    }

    /// A head submitted to this node. A version 2 head an introduction keeps
    /// for old nodes counts as kept.
    pub(crate) fn accept(&mut self, candidate: SeedDocument, tip: Option<u32>) -> Result<u64, String> {
        let sequence = candidate.sequence;
        let legacy = candidate.version == 2 && self.keep_legacy(&candidate)?;
        match self.weigh(candidate, tip)? {
            Weighed::Kept => Ok(sequence),
            _ if legacy => Ok(sequence),
            Weighed::Unchanged => Err("an older sequence does not replace the document".into()),
            Weighed::Waiting => Err("that head drops fee terms that may still list a store; try again later".into()),
            Weighed::Refused(reason) => Err(reason),
        }
    }

    fn weigh(&mut self, candidate: SeedDocument, tip: Option<u32>) -> Result<Weighed, String> {
        if candidate.version == 2 {
            self.keep_legacy(&candidate)?;
        }
        let Some(held) = &self.document else {
            self.store(candidate)?;
            return Ok(Weighed::Kept);
        };
        if candidate == *held || candidate.sequence < held.sequence || (candidate.version == 2 && held.version != 2) {
            return Ok(Weighed::Unchanged);
        }
        if let Some((_, reason)) = self.refused.as_ref().filter(|(sequence, _)| *sequence == candidate.sequence) {
            return Ok(Weighed::Refused(reason.clone()));
        }
        let upgrade = held.version == 2 && candidate.version != 2 && candidate.introductions == held.introductions;
        if candidate.sequence == held.sequence && !upgrade {
            let bytes = candidate.encode().map_err(|err| err.to_string())?;
            atomic(&self.dir.join(format!("equivocation-{}.obsd", candidate.sequence)), &bytes)?;
            return self.refuse(candidate.sequence, "two different heads were signed at this sequence");
        }
        let view = HeadView { tip, last_current: self.last_current };
        match held.schedule.successor(&candidate.schedule, view) {
            Ok(()) => {
                self.store(candidate)?;
                Ok(Weighed::Kept)
            }
            Err(Refusal::NotYet(_)) => Ok(Weighed::Waiting),
            Err(Refusal::Rewrites(reason)) => self.refuse(candidate.sequence, reason),
        }
    }

    fn store(&mut self, document: SeedDocument) -> Result<(), String> {
        let bytes = document.encode().map_err(|err| err.to_string())?;
        atomic(&self.dir.join("document.obsd"), &bytes)?;
        if self.refused.as_ref().is_some_and(|(sequence, _)| document.sequence > *sequence) {
            self.refused = None;
            let _ = fs::remove_file(self.dir.join("head-refused"));
        }
        self.document = Some(document);
        Ok(())
    }

    fn keep_legacy(&mut self, document: &SeedDocument) -> Result<bool, String> {
        if self.legacy.as_ref().is_some_and(|held| held.sequence >= document.sequence) {
            return Ok(false);
        }
        atomic(&self.dir.join("document-v2.obsd"), &document.encode().map_err(|err| err.to_string())?)?;
        self.legacy = Some(document.clone());
        Ok(true)
    }

    fn refuse(&mut self, sequence: u64, reason: &str) -> Result<Weighed, String> {
        if self.refused.as_ref().map(|(held, _)| *held) != Some(sequence) {
            eprintln!("ob-node: refused directory head sequence {sequence}: {reason}");
            atomic(&self.dir.join("head-refused"), format!("{sequence}\n{reason}\n").as_bytes())?;
            self.refused = Some((sequence, reason.to_string()));
        }
        Ok(Weighed::Refused(reason.to_string()))
    }
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

#[cfg(test)]
#[path = "directory_head_tests.rs"]
mod tests;
