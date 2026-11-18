//! The moderator's desk (testnet4 only, `EscrowConfig::moderates`).
//!
//! The node answers for two identities: the built-in test moderator, whose
//! keys are public, and its own ([`crate::moderator_identity`]). A letter is
//! reviewed under the record its terms name, and the case remembers which
//! record that was. A case with none is the built-in moderator's, which is
//! what every case opened before a node had its own identity is.
//!
//! Reviewing needs no moderator secret, so letters are checked while the
//! wallet is locked. Ruling signs with the identity's keys, so a ruling
//! under the node's own identity needs the wallet unlocked.
//!
//! A dispute letter, from the buyer or the vendor, is reviewed when it
//! arrives: the escrow crate checks the terms, the claimant's signature, the
//! deadline against this node's tip, the rate limits ([`DisputeLimits`]),
//! the funding output against this node's chain source, and the proofs. The
//! case then waits for the operator, who rules with
//! `POST /moderator/disputes/{id}/rule`. A letter can only be ruled for the
//! branch it asks for.
//!
//! The chain check follows the funding script through
//! [`ChainSource::script_txs`] rather than looking the txid up, so it works
//! on compact block filters, which cannot look a transaction up by txid.
//! While the filters are still scanning for that script the letter stays in
//! the inbox and is reviewed again.
//!
//! Cases are durable records ([`crate::escrow_store`]), one per escrow and
//! branch. Rulings go through [`FileRulingStore`], which records at most one
//! branch per escrow across restarts and crashes before anything is
//! decrypted. A ruling is stored with its case before it is sent, so a crash
//! after the ruling resends the same branch.
//!
//! A letter under v5 terms owes the moderator a filing fee ([`fee`]). The
//! operator can rule on it or deny it (`POST /moderator/disputes/{id}/deny`)
//! only once the escrow's fee is paid or when the fee is zero. Either one
//! is signed as a public decision; a denial is final.

mod case;
mod decision;
mod fee;

use std::collections::BTreeMap;
use std::sync::MutexGuard;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::{OutPoint, Script, TxOut};
use openbazaar_escrow::{
    review_letter, Decision, DisputeCase, Error as EscrowError, FeeCase, FundingChain, Moderator, ModeratorKeys,
    ModeratorRecord, ModeratorRuling, Party,
};
use serde_json::{json, Value};
use tormail::StoreKey;

use self::case::{branch_byte, case_name, check_citation, citation_from_fields, party_name, party_of, Case, CaseFee};
use self::fee::{fee_id, EscrowFee, FeePool};
use crate::chain::ChainSource;
use crate::escrow_fixture::{legacy_moderator_record, moderator_keys, moderator_record};
use crate::escrow_limits::DisputeLimits;
use crate::escrow_mail::{hex, id_hex, key_bytes, parse_id, EscrowId, Kind, Mail, RETRY_LATER};
use crate::escrow_outbox::now_secs;
use crate::escrow_rulings::FileRulingStore;
use crate::escrow_store::{EscrowStore, Role};
use crate::pay_order::PayError;
use crate::Node;

/// Cases by escrow and the branch the letter asks for, and the filing fee
/// of each v5 escrow that owes one.
#[derive(Default)]
pub(crate) struct ModeratorDesk {
    cases: BTreeMap<(EscrowId, u8), Case>,
    fees: BTreeMap<EscrowId, EscrowFee>,
    pool: FeePool,
    fee_tick: u64,
}

impl ModeratorDesk {
    pub(crate) fn load(store: &EscrowStore) -> Self {
        let mut desk = Self::default();
        for (name, record) in store.list(Role::Moderator) {
            if FeePool::is_pool(&name) {
                match FeePool::from_record(&record) {
                    Some(pool) => desk.pool = pool,
                    None => eprintln!("ob-node: moderator fee pool could not be read"),
                }
                continue;
            }
            if let Some(id) = fee_id(&name) {
                match EscrowFee::from_record(&record) {
                    Some(fee) => {
                        desk.fees.insert(id, fee);
                    }
                    None => eprintln!("ob-node: moderator fee {name} could not be read"),
                }
                continue;
            }
            let parsed = name.split_once('-').and_then(|(id, party)| Some((parse_id(id)?, branch_byte(party)?)));
            match (parsed, Case::from_record(&record)) {
                (Some(key), Some(case)) => {
                    desk.cases.insert(key, case);
                }
                _ => eprintln!("ob-node: moderator case {name} could not be read"),
            }
        }
        desk
    }
}

/// This node's chain source, followed by script. Confirmed outputs only.
pub(crate) struct ChainView<'a>(pub &'a dyn ChainSource);

impl FundingChain for ChainView<'_> {
    fn funding_output(&self, outpoint: &OutPoint, script: &Script, not_before: u32) -> Result<Option<TxOut>, EscrowError> {
        let txs = self.0.script_txs(script, not_before).map_err(|err| {
            eprintln!("ob-node: moderator chain view: {err}");
            EscrowError::ChainNotReady
        })?;
        Ok(txs
            .iter()
            .filter(|tx| tx.txid == outpoint.txid && tx.height.is_some())
            .flat_map(|tx| tx.outputs.iter())
            .find(|(vout, _)| *vout == outpoint.vout)
            .map(|(_, sats)| TxOut {
                value: bitcoin::Amount::from_sat(*sats),
                script_pubkey: script.to_owned(),
            }))
    }
}

fn letter_digest(letter: &[u8]) -> [u8; 32] {
    sha256::Hash::hash(letter).to_byte_array()
}

/// The filing fee of a reviewed letter's terms; `None` under v4 terms.
fn case_fee(case: &DisputeCase) -> Option<CaseFee> {
    let terms = case.terms();
    let amount = terms.moderator.filing_fee()?;
    let fee_case = FeeCase {
        terms,
        outpoint: case.funding_outpoint(),
        filer: case.favored(),
        letter_digest: [0; 32],
    };
    Some(CaseFee {
        case_id: fee_case.case_id(),
        amount: amount.to_sat(),
        due: terms.vendor_claim_height,
    })
}

impl Node {
    fn moderator_desk(&self) -> MutexGuard<'_, ModeratorDesk> {
        self.escrow
            .moderator
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// The keys a case is ruled under: this node's own identity for a case
    /// that names one of its records, else the built-in one.
    fn case_keys(&self, record: Option<&[u8]>) -> Result<ModeratorKeys, String> {
        match record {
            Some(_) => self.own_moderator_keys(),
            None => moderator_keys(self.network),
        }
    }

    /// The moderator a case is ruled by. A built-in case with a filing fee
    /// was reviewed under the built-in filing-fee record, one without under
    /// its record from before filing fees.
    fn moderator(&self, record: Option<&[u8]>, filing: bool) -> Result<Moderator<ChainView<'_>, &FileRulingStore>, String> {
        let keys = self.case_keys(record)?;
        let record = match (record, filing) {
            (Some(bytes), _) => ModeratorRecord::from_bytes(bytes).map_err(|err| err.to_string())?,
            (None, true) => moderator_record(self.network)?,
            (None, false) => legacy_moderator_record(self.network)?,
        };
        Moderator::new(keys, record, ChainView(&*self.chain), &self.escrow.rulings)
            .map(|moderator| moderator.with_cache(self.escrow.letters.clone()))
            .map_err(|err| err.to_string())
    }

    /// Review `letter` under the record its terms name: the built-in
    /// moderator's, or one this node's own identity has published. Returns
    /// the case and that record's bytes (`None` for the built-in one).
    fn review_any(&self, letter: &[u8], received: u32) -> Result<(DisputeCase, Option<Vec<u8>>), EscrowError> {
        let digest = letter_digest(letter);
        let limits: &DisputeLimits = &self.escrow.limits;
        let mut gate = |key: &openbazaar_escrow::RulingKey, _: Party| limits.admit(digest, key, now_secs());
        let builtin = [moderator_record(self.network), legacy_moderator_record(self.network)]
            .into_iter()
            .filter_map(Result::ok)
            .map(|record| (record, None));
        let own = self.own_moderator_records().into_iter().map(|record| {
            let bytes = record.to_bytes();
            (record, Some(bytes))
        });
        for (record, bytes) in builtin.into_iter().chain(own) {
            match review_letter(&record, &ChainView(&*self.chain), &self.escrow.letters, letter, received, &mut gate) {
                Err(EscrowError::ModeratorMismatch) => continue,
                result => return result.map(|case| (case, bytes)),
            }
        }
        Err(EscrowError::ModeratorMismatch)
    }

    pub(crate) fn moderator_mail(&self, mail: Mail) -> Result<(), String> {
        let writer = match mail.kind {
            Kind::Dispute => Party::Buyer,
            Kind::VendorDispute => Party::Vendor,
            other => return Err(format!("a moderator does not take {other:?}")),
        };
        self.moderator_review(&mail, writer)
    }

    /// Review a letter from `writer`, who must be the party it favors.
    fn moderator_review(&self, mail: &Mail, writer: Party) -> Result<(), String> {
        let fields = mail.as_fields();
        let citation = match fields.len() {
            3 => None,
            6 => Some(citation_from_fields(fields)?),
            _ => return Err("a Dispute message carries 3 fields".into()),
        };
        let reply_mail = key_bytes(&fields[1])?;
        let reply_url = crate::peer_http::sale_base(&String::from_utf8(fields[2].clone()).map_err(|err| err.to_string())?)?;
        let received = self.chain.tip_height()?;
        let (case, record) = match self.review_any(&fields[0], received) {
            Ok(reviewed) => reviewed,
            Err(EscrowError::ChainNotReady) => return Err(format!("{RETRY_LATER}the funding is not checked yet")),
            Err(err) => return Err(err.to_string()),
        };
        if case.favored() != writer {
            return Err("a dispute letter must ask for its writer's branch".into());
        }
        if let Some(citation) = citation {
            check_citation(&mail.escrow, case.terms(), &citation)?;
        }
        let key = (mail.escrow, case.favored() as u8);
        let fee = case_fee(&case);
        let mut desk = self.moderator_desk();
        if desk.cases.contains_key(&key) {
            drop(desk);
            return self.open_case_fee(&mail.escrow, writer, fee);
        }
        let case = Case {
            letter: fields[0].clone(),
            memo: String::from_utf8_lossy(case.memo()).into_owned(),
            outpoint: case.funding_outpoint(),
            received,
            reply_mail,
            reply_url,
            ruling: None,
            citation,
            public_ruling: None,
            outcome: None,
            record,
            tracking: None,
            fee,
            decision: None,
        };
        self.escrow
            .store
            .save(Role::Moderator, &case_name(&mail.escrow, party_of(key.1)), &case.to_record())?;
        desk.cases.insert(key, case);
        drop(desk);
        self.open_case_fee(&mail.escrow, writer, fee)
    }

    fn open_case_fee(&self, id: &EscrowId, filer: Party, fee: Option<CaseFee>) -> Result<(), String> {
        match fee {
            Some(fee) if fee.amount > 0 => self.open_fee(id, filer, fee.case_id, fee.amount, fee.due),
            _ => Ok(()),
        }
    }

    /// True when this desk holds a letter for the escrow, in either branch.
    pub(crate) fn dispute_case_known(&self, id: &EscrowId) -> bool {
        let desk = self.moderator_desk();
        desk.cases.keys().any(|(case_id, _)| case_id == id)
    }

    /// Record this node's own shipment lookup on every letter of the escrow,
    /// so the evidence survives a restart beside the case it informs.
    pub(crate) fn dispute_tracking_store(&self, id: &EscrowId, tracking: &Value) -> Result<(), String> {
        let mut desk = self.moderator_desk();
        let mut found = false;
        for ((case_id, branch), case) in desk.cases.iter_mut() {
            if case_id != id {
                continue;
            }
            case.tracking = Some(tracking.clone());
            self.escrow
                .store
                .save(Role::Moderator, &case_name(case_id, party_of(*branch)), &case.to_record())?;
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err("the case is gone".to_string())
        }
    }

    pub(crate) fn disputes_value(&self) -> Value {
        let tip = self.chain.tip_height().ok();
        let desk = self.moderator_desk();
        let decided = |id: &EscrowId| {
            desk.cases
                .iter()
                .any(|((case_id, _), case)| case_id == id && (case.ruling.is_some() || case.decision.is_some()))
        };
        let cases: Vec<Value> = desk
            .cases
            .iter()
            .map(|((id, branch), case)| {
                let favored = party_of(*branch);
                json!({
                    "id": id_hex(id),
                    "favors": party_name(favored),
                    "memo": case.memo,
                    "outpoint": case.outpoint.to_string(),
                    "receivedHeight": case.received,
                    "ruled": case.ruling.is_some(),
                    "denied": case.ruling.is_none() && case.decision.is_some(),
                    "builtIn": case.record.is_none(),
                    "tracking": case.tracking.clone(),
                    "caseId": case.fee.map(|fee| hex(&fee.case_id)),
                    "fee": desk.case_fee_value(id, case, tip, decided(id)),
                })
            })
            .collect();
        json!({ "disputes": cases })
    }

    /// The public rulings this node's own identity signed, as hex. The
    /// built-in moderator's are left out: its keys are public, so its
    /// rulings prove nothing about this node.
    pub(crate) fn public_rulings_value(&self) -> Vec<Value> {
        let desk = self.moderator_desk();
        desk.cases
            .values()
            .filter(|case| case.record.is_some())
            .filter_map(|case| case.public_ruling.as_deref())
            .take(crate::moderators::MAX_RULINGS)
            .map(|ruling| json!(hex(ruling)))
            .collect()
    }

    /// The signed dispute outcomes this node's own identity signed, as hex.
    /// The built-in moderator's are left out for the same reason as its
    /// rulings: its keys are public, so they prove nothing about this node.
    pub(crate) fn dispute_outcomes_value(&self) -> Vec<Value> {
        let desk = self.moderator_desk();
        desk.cases
            .values()
            .filter(|case| case.record.is_some())
            .filter_map(|case| case.outcome.as_deref())
            .take(crate::moderators::MAX_RULINGS)
            .map(|outcome| json!(hex(outcome)))
            .collect()
    }

    /// Rule on a reviewed letter and send the ruling to the party that wrote it.
    pub fn rule_dispute(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let favor = favor_of(body)?;
        let escrow_id = parse_id(id).ok_or(PayError::NotFound)?;
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (letter, received, record, filing) = {
            let desk = self.moderator_desk();
            let case = desk.undecided(&escrow_id, favor)?;
            (case.letter.clone(), case.received, case.record.clone(), case.fee.is_some())
        };
        let ruling = self
            .moderator(record.as_deref(), filing)
            .map_err(PayError::Failed)?
            .open(&letter, received)
            .map_err(|err| PayError::Conflict(err.to_string()))?;
        let (url, to) = self.store_ruling(&escrow_id, favor, &ruling).map_err(PayError::Failed)?;
        self.send_mail(&url, &to, &Mail::new(Kind::Ruling, escrow_id, vec![ruling.to_bytes()]))
            .map_err(|err| PayError::Chain(format!("The party's node refused the ruling: {err}")))?;
        Ok(json!({ "id": id, "favors": party_name(favor), "ruled": true }))
    }

    fn store_ruling(&self, id: &EscrowId, favor: Party, ruling: &ModeratorRuling) -> Result<(String, StoreKey), String> {
        let mut desk = self.moderator_desk();
        let case = desk.cases.get_mut(&(*id, favor as u8)).ok_or("the case is gone")?;
        case.ruling = Some(ruling.to_bytes());
        let keys = self.case_keys(case.record.as_deref())?;
        case.outcome = Some(keys.sign_dispute_outcome(&ruling.to_bytes()).map_err(|err| err.to_string())?);
        if let Some(citation) = case.citation {
            case.public_ruling = Some(
                keys.sign_public_ruling(&ruling.to_bytes(), citation.store_key, citation.listing_id)
                    .map_err(|err| err.to_string())?,
            );
        }
        if let Some(fee) = case.fee {
            let decision = keys.sign_dispute_decision(fee.case_id, Decision::Ruled(favor), Some(&ruling.to_bytes()));
            case.decision = Some(decision.map_err(|err| err.to_string())?);
        }
        self.escrow.store.save(Role::Moderator, &case_name(id, favor), &case.to_record())?;
        Ok((case.reply_url.clone(), case.reply_mail))
    }
}

/// The branch a rule or deny request names.
fn favor_of(body: &Value) -> Result<Party, PayError> {
    let favor = match body.as_object() {
        Some(map) if map.len() == 1 => map.get("favor").and_then(Value::as_str),
        _ => None,
    };
    match favor {
        Some("buyer") => Ok(Party::Buyer),
        Some("vendor") => Ok(Party::Vendor),
        _ => Err(PayError::Invalid("Send {\"favor\": \"buyer\"} or {\"favor\": \"vendor\"}.".into())),
    }
}

impl ModeratorDesk {
    /// The letter for `favor` the operator may still decide: not ruled,
    /// not denied, and with the escrow's filing fee settled.
    fn undecided(&self, id: &EscrowId, favor: Party) -> Result<&Case, PayError> {
        let Some(case) = self.cases.get(&(*id, favor as u8)) else {
            let other = match favor {
                Party::Buyer => Party::Vendor,
                Party::Vendor => Party::Buyer,
            };
            if self.cases.contains_key(&(*id, other as u8)) {
                let asks = party_name(other);
                return Err(PayError::Invalid(format!(
                    "This letter asks for the {asks}. Only the {asks} branch can be opened from it."
                )));
            }
            return Err(PayError::NotFound);
        };
        if case.ruling.is_some() {
            return Err(PayError::Conflict("This dispute has a ruling.".into()));
        }
        if case.decision.is_some() {
            return Err(PayError::Conflict("This letter was denied. A denial is final.".into()));
        }
        if !self.fee_settled(id, case) {
            return Err(PayError::Conflict("The filing fee is not paid yet.".into()));
        }
        Ok(case)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::MockChain;

    #[test]
    fn the_chain_view_finds_funding_by_script_and_waits_while_filters_scan() {
        let chain = MockChain::new();
        chain.0.filters.store(true, std::sync::atomic::Ordering::Relaxed);
        let script = bitcoin::ScriptBuf::new_p2a();
        let address = bitcoin::Address::from_script(&script, bitcoin::Network::Testnet4).unwrap();
        let txid = chain.pay_external(&address.to_string(), 5_000);
        let outpoint = OutPoint { txid, vout: 0 };
        let view = ChainView(&chain);
        assert!(chain.output(&outpoint).is_err(), "a filter backend cannot look the txid up");
        assert_eq!(view.funding_output(&outpoint, &script, 7), Err(EscrowError::ChainNotReady));
        assert_eq!(view.funding_output(&outpoint, &script, 7), Ok(None), "unconfirmed");
        chain.mine();
        let found = view.funding_output(&outpoint, &script, 7).unwrap().unwrap();
        assert_eq!(found.value.to_sat(), 5_000);
        assert_eq!(view.funding_output(&OutPoint { txid, vout: 1 }, &script, 7), Ok(None));
        assert_eq!(*chain.0.followed.lock().unwrap(), vec![(script, 7)]);
    }
}
