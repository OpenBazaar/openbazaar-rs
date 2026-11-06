//! The fixture moderator's desk (testnet4 only, `EscrowConfig::moderates`).
//!
//! A dispute letter is reviewed when it arrives: the escrow crate checks the
//! terms, the deadline against this node's tip, and the funding output
//! against this node's chain source. The case then waits for the operator,
//! who rules with `POST /moderator/disputes/{id}/rule`. A letter can only be
//! ruled for the branch it asks for, and an escrow is ruled at most once.
//!
//! Rulings are recorded in memory, so a restart forgets which escrows were
//! ruled. That is one reason this moderator is a testnet fixture.

use std::collections::{BTreeMap, HashMap};
use std::sync::MutexGuard;

use bitcoin::{OutPoint, TxOut};
use openbazaar_escrow::{FundingChain, Moderator, Party, RulingKey, RulingStore};
use serde_json::{json, Value};
use tormail::StoreKey;

use crate::chain::ChainSource;
use crate::escrow_fixture::{moderator_keys, moderator_record};
use crate::escrow_mail::{check_loopback, id_hex, key_bytes, parse_id, EscrowId, Kind, Mail};
use crate::pay_order::PayError;
use crate::Node;

struct Case {
    letter: Vec<u8>,
    favored: Party,
    memo: String,
    outpoint: OutPoint,
    received: u32,
    reply_mail: StoreKey,
    reply_url: String,
    ruled: bool,
}

#[derive(Default)]
pub(crate) struct ModeratorDesk {
    cases: BTreeMap<EscrowId, Case>,
    rulings: HashMap<RulingKey, Party>,
}

/// Confirmed outputs only, from this node's own chain source.
struct ChainView<'a>(&'a dyn ChainSource);

impl FundingChain for ChainView<'_> {
    fn funding_output(&self, outpoint: &OutPoint) -> Option<TxOut> {
        match self.0.output(outpoint) {
            Ok(Some((out, Some(_height)))) => Some(out),
            _ => None,
        }
    }
}

struct Rulings<'a>(&'a mut HashMap<RulingKey, Party>);

impl RulingStore for Rulings<'_> {
    fn record_once(
        &mut self,
        key: RulingKey,
        favored: Party,
    ) -> Result<Party, openbazaar_escrow::Error> {
        Ok(*self.0.entry(key).or_insert(favored))
    }
}

fn party_name(party: Party) -> &'static str {
    match party {
        Party::Buyer => "buyer",
        Party::Vendor => "vendor",
    }
}

impl Node {
    fn moderator_desk(&self) -> MutexGuard<'_, ModeratorDesk> {
        self.escrow
            .moderator
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn moderator<'a>(
        &'a self,
        rulings: &'a mut HashMap<RulingKey, Party>,
    ) -> Result<Moderator<ChainView<'a>, Rulings<'a>>, String> {
        Moderator::new(
            moderator_keys(self.network)?,
            moderator_record(self.network)?,
            ChainView(&*self.chain),
            Rulings(rulings),
        )
        .map_err(|err| err.to_string())
    }

    pub(crate) fn moderator_mail(&self, mail: Mail) -> Result<(), String> {
        if mail.kind != Kind::Dispute {
            return Err(format!("a moderator does not take {:?}", mail.kind));
        }
        let fields = mail.fields(3)?;
        let reply_mail = key_bytes(&fields[1])?;
        let reply_url = String::from_utf8(fields[2].clone()).map_err(|err| err.to_string())?;
        check_loopback(&reply_url)?;
        let received = self.chain.tip_height()?;
        let mut desk = self.moderator_desk();
        if desk.cases.contains_key(&mail.escrow) {
            return Err("that escrow already has a dispute".into());
        }
        let ModeratorDesk { cases, rulings } = &mut *desk;
        let case = self
            .moderator(rulings)?
            .review(&fields[0], received)
            .map_err(|err| err.to_string())?;
        cases.insert(
            mail.escrow,
            Case {
                letter: fields[0].clone(),
                favored: case.favored(),
                memo: String::from_utf8_lossy(case.memo()).into_owned(),
                outpoint: case.funding_outpoint(),
                received,
                reply_mail,
                reply_url,
                ruled: false,
            },
        );
        Ok(())
    }

    pub(crate) fn disputes_value(&self) -> Value {
        let desk = self.moderator_desk();
        let cases: Vec<Value> = desk
            .cases
            .iter()
            .map(|(id, case)| {
                json!({
                    "id": id_hex(id),
                    "favors": party_name(case.favored),
                    "memo": case.memo,
                    "outpoint": case.outpoint.to_string(),
                    "receivedHeight": case.received,
                    "ruled": case.ruled,
                })
            })
            .collect();
        json!({ "disputes": cases })
    }

    /// Rule on a reviewed letter and send the ruling to the party that wrote it.
    pub fn rule_dispute(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let favor = match body.as_object() {
            Some(map) if map.len() == 1 => map.get("favor").and_then(Value::as_str),
            _ => None,
        };
        let favor = match favor {
            Some("buyer") => Party::Buyer,
            Some("vendor") => Party::Vendor,
            _ => {
                return Err(PayError::Invalid(
                    "Send {\"favor\": \"buyer\"} or {\"favor\": \"vendor\"}.".into(),
                ))
            }
        };
        let escrow_id = parse_id(id).ok_or(PayError::NotFound)?;
        let (ruling, url, to) = {
            let mut desk = self.moderator_desk();
            let ModeratorDesk { cases, rulings } = &mut *desk;
            let case = cases.get_mut(&escrow_id).ok_or(PayError::NotFound)?;
            if case.favored != favor {
                let asks = party_name(case.favored);
                return Err(PayError::Invalid(format!(
                    "This letter asks for the {asks}. Only the {asks} branch can be opened from it."
                )));
            }
            if case.ruled {
                return Err(PayError::Conflict("This dispute has a ruling.".into()));
            }
            let ruling = self
                .moderator(rulings)
                .map_err(PayError::Failed)?
                .open(&case.letter, case.received)
                .map_err(|err| PayError::Conflict(err.to_string()))?;
            case.ruled = true;
            (ruling, case.reply_url.clone(), case.reply_mail)
        };
        self.send_mail(
            &url,
            &to,
            &Mail::new(Kind::Ruling, escrow_id, vec![ruling.to_bytes()]),
        )
        .map_err(|err| PayError::Chain(format!("The party's node could not be reached: {err}")))?;
        Ok(json!({ "id": id, "favors": party_name(favor), "ruled": true }))
    }
}
