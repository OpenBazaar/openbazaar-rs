//! What `ob-node sign-seed` signs: the next directory head, built on the one
//! introductions serve now.
//!
//! A new entry inherits every term it does not name from the last entry and
//! starts at least [`notice_blocks`] after the operator's tip. The fee key is
//! the last entry's: this build does not sign a key change. Entries drop off
//! the front with `--prune` once they retired [`SKEW_BLOCKS`] before the tip.
//! The finished head is checked with the same [`TermsSchedule::successor`]
//! nodes run, so a head published within [`SKEW_BLOCKS`] of the tip is kept.

use bitcoin::secp256k1::{Secp256k1, SecretKey};
use openbazaar_protocol::{
    notice_blocks, DirectoryTerms, HeadView, SeedDocument, TermsEntry, TermsSchedule, EPOCH_BLOCKS, FEE_CONFS, FEE_SATS, MAX_TERMS,
    SKEW_BLOCKS,
};

use crate::seeds::{DOCUMENT_KEY, INTRODUCTIONS};

/// One `sign-seed` run.
#[derive(Clone, Debug, Default)]
pub struct HeadPlan {
    pub sequence: u64,
    /// Empty means the two compiled onions.
    pub introductions: Vec<String>,
    /// The head introductions serve now, version 2 or 3. `None` signs the first head.
    pub previous: Option<Vec<u8>>,
    /// The operator's chain tip. Becomes the announced height of an added entry.
    pub tip: Option<u32>,
    pub append: Option<TermsChange>,
    pub prune: bool,
    /// Also sign the version 2 head for nodes that do not read version 3.
    pub v2: bool,
}

/// An entry to add. A field left out keeps the last entry's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermsChange {
    pub start_height: u32,
    pub fee_sats: Option<u64>,
    pub fee_confs: Option<u32>,
    pub epoch_blocks: Option<u32>,
}

impl TermsChange {
    /// `start=H[,sats=N][,confs=C][,epoch=E]`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut change = TermsChange::default();
        let mut start = None;
        for pair in text.split(',').map(str::trim).filter(|pair| !pair.is_empty()) {
            let (name, value) = pair.split_once('=').ok_or_else(|| format!("{pair} is not name=value"))?;
            let bad = || format!("{name}={value} is not a number");
            match name {
                "start" => start = Some(value.parse().map_err(|_| bad())?),
                "sats" => change.fee_sats = Some(value.parse().map_err(|_| bad())?),
                "confs" => change.fee_confs = Some(value.parse().map_err(|_| bad())?),
                "epoch" => change.epoch_blocks = Some(value.parse().map_err(|_| bad())?),
                "fee-key" | "key" => return Err("this build does not change the fee key".into()),
                other => return Err(format!("unknown field {other}")),
            }
        }
        change.start_height = start.ok_or("start=H is required")?;
        Ok(change)
    }
}

#[derive(Debug)]
pub struct SignedHeads {
    pub v3: Vec<u8>,
    pub v2: Option<Vec<u8>>,
    /// Lines for the operator: the schedule, and when to publish by.
    pub summary: Vec<String>,
}

/// Sign the head `plan` describes with the operator secret. Refuses a secret
/// whose public key is not [`DOCUMENT_KEY`].
pub fn sign_head(plan: HeadPlan, secret_bytes: [u8; 32]) -> Result<SignedHeads, String> {
    let secret = SecretKey::from_slice(&secret_bytes).map_err(|_| "the document key is not a secret key".to_string())?;
    if secret.x_only_public_key(&Secp256k1::new()).0.serialize() != DOCUMENT_KEY {
        return Err("this key does not match the public key shipped in the binary".into());
    }
    sign_with(&plan, &secret)
}

/// [`sign_head`] under whatever key `secret` is. The fee key is that key.
pub(crate) fn sign_with(plan: &HeadPlan, secret: &SecretKey) -> Result<SignedHeads, String> {
    let key = secret.x_only_public_key(&Secp256k1::new()).0.serialize();
    let previous = match &plan.previous {
        Some(bytes) => {
            let document = SeedDocument::decode(bytes).map_err(|err| format!("the previous head: {err}"))?;
            document.verify_signer(&key).map_err(|_| "the previous head is not signed by the shipped key".to_string())?;
            if plan.sequence <= document.sequence {
                return Err(format!("the sequence must be above the previous head's {}", document.sequence));
            }
            Some(document)
        }
        None => None,
    };
    let schedule = plan_schedule(plan, previous.as_ref(), key)?;
    if let Some(previous) = &previous {
        let view = HeadView { tip: plan.tip, last_current: plan.tip.unwrap_or(0) };
        previous.schedule.successor(&schedule, view).map_err(|refusal| format!("nodes would refuse this head: {}", refusal.reason()))?;
    }
    let introductions = if plan.introductions.is_empty() {
        INTRODUCTIONS.iter().map(|onion| (*onion).to_string()).collect()
    } else {
        plan.introductions.clone()
    };
    let v2 = if plan.v2 {
        if !schedule.entries().iter().all(|entry| keeps_version_2(&entry.terms, key)) {
            return Err("--v2-out: nodes that read only version 2 judge every fee by 20,000 sats, 1 confirmation, and 2,016 blocks; \
                        terms below that would hide stores from them"
                .into());
        }
        let document = SeedDocument::sign_v2(plan.sequence, introductions.clone(), DirectoryTerms::standard(key), None, secret).map_err(|err| err.to_string())?;
        Some(document.encode().map_err(|err| err.to_string())?)
    } else {
        None
    };
    let document = SeedDocument::sign(plan.sequence, introductions, schedule, None, secret).map_err(|err| err.to_string())?;
    let mut summary = describe_head(&document);
    if let (Some(tip), Some(_)) = (plan.tip, plan.append) {
        summary.push(format!("Publish to every introduction before block {}.", tip.saturating_add(SKEW_BLOCKS)));
    }
    Ok(SignedHeads { v3: document.encode().map_err(|err| err.to_string())?, v2, summary })
}

/// One line per entry of `document`'s schedule.
pub fn describe_head(document: &SeedDocument) -> Vec<String> {
    document
        .schedule
        .entries()
        .iter()
        .map(|entry| {
            let terms = &entry.terms;
            format!(
                "from block {}: {} sats, {} confirmation{}, {}-block epoch (announced at block {})",
                entry.start_height,
                terms.fee_sats,
                terms.fee_confs,
                if terms.fee_confs == 1 { "" } else { "s" },
                terms.epoch_blocks,
                entry.announced_height
            )
        })
        .collect()
}

fn keeps_version_2(terms: &DirectoryTerms, key: [u8; 32]) -> bool {
    terms.fee_key == key && terms.fee_sats >= FEE_SATS && terms.fee_confs >= FEE_CONFS && terms.epoch_blocks <= EPOCH_BLOCKS
}

fn plan_schedule(plan: &HeadPlan, previous: Option<&SeedDocument>, key: [u8; 32]) -> Result<TermsSchedule, String> {
    let mut entries = previous.map_or_else(|| vec![TermsEntry::genesis(DirectoryTerms::standard(key))], |document| document.schedule.entries().to_vec());
    if plan.append.is_none() && !plan.prune {
        return TermsSchedule::new(entries).map_err(|err| err.to_string());
    }
    let tip = plan.tip.ok_or("--append-terms and --prune need --tip, the chain tip now")?;
    if let Some(announced) = entries.iter().map(|entry| entry.announced_height).max().filter(|announced| tip < *announced) {
        return Err(format!("--tip {tip} is below the previous head's announced block {announced}"));
    }
    if plan.prune {
        let held = TermsSchedule::new(entries.clone()).map_err(|err| err.to_string())?;
        entries.drain(..held.prunable(tip.saturating_sub(SKEW_BLOCKS)));
    }
    if let Some(change) = plan.append {
        let last = *entries.last().ok_or("the previous head has no fee terms")?;
        let terms = DirectoryTerms {
            fee_key: last.terms.fee_key,
            fee_sats: change.fee_sats.unwrap_or(last.terms.fee_sats),
            fee_confs: change.fee_confs.unwrap_or(last.terms.fee_confs),
            epoch_blocks: change.epoch_blocks.unwrap_or(last.terms.epoch_blocks),
        };
        let notice = notice_blocks(&last.terms, &terms);
        let earliest = tip.saturating_add(notice).max(last.start_height.saturating_add(1));
        if change.start_height < earliest {
            return Err(format!("the earliest start for these terms is block {earliest} ({notice} blocks of notice after the tip)"));
        }
        if entries.len() >= MAX_TERMS {
            return Err(format!("a head schedules at most {MAX_TERMS} fee terms; add --prune once older terms have retired"));
        }
        entries.push(TermsEntry { start_height: change.start_height, announced_height: tip, terms });
    }
    TermsSchedule::new(entries).map_err(|err| err.to_string())
}

#[cfg(test)]
#[path = "head_plan_tests.rs"]
mod tests;
