//! Denying a v5 letter, and the public decisions this node signed.
//!
//! A denial closes one party's letter without a ruling: the moderator keeps
//! the filing fee and decrypts nothing. It is signed as a public decision
//! ([`openbazaar_protocol::DisputeDecision`]), so a later ruling for that
//! same party is provable misconduct
//! ([`openbazaar_escrow::is_denial_reversed`]). The other party's letter, if
//! any, can still be ruled on.

use openbazaar_escrow::Decision;
use serde_json::{json, Value};

use super::case::{case_name, party_name};
use super::favor_of;
use crate::escrow_mail::{hex, parse_id, EscrowId, Kind, Mail};
use crate::escrow_store::Role;
use crate::pay_order::PayError;
use crate::Node;

impl Node {
    /// Deny the letter asking for `favor` and tell the party that wrote it.
    pub fn deny_dispute(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let favor = favor_of(body)?;
        let escrow_id = parse_id(id).ok_or(PayError::NotFound)?;
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (record, case_id) = {
            let desk = self.moderator_desk();
            let case = desk.undecided(&escrow_id, favor)?;
            let fee = case.fee.ok_or_else(|| {
                PayError::Invalid("This letter has no filing fee, so it can only be ruled on.".into())
            })?;
            (case.record.clone(), fee.case_id)
        };
        let keys = self.case_keys(record.as_deref()).map_err(PayError::Failed)?;
        let decision = keys
            .sign_dispute_decision(case_id, Decision::Denied(favor), None)
            .map_err(|err| PayError::Failed(err.to_string()))?;
        let (url, to) = {
            let mut desk = self.moderator_desk();
            let case = desk
                .cases
                .get_mut(&(escrow_id, favor as u8))
                .ok_or(PayError::NotFound)?;
            case.decision = Some(decision.clone());
            self.escrow
                .store
                .save(Role::Moderator, &case_name(&escrow_id, favor), &case.to_record())
                .map_err(PayError::Failed)?;
            (case.reply_url.clone(), case.reply_mail)
        };
        self.send_mail(&url, &to, &Mail::new(Kind::Denied, escrow_id, vec![decision]))
            .map_err(|err| PayError::Chain(format!("The party's node refused the denial: {err}")))?;
        Ok(json!({ "id": id, "favors": party_name(favor), "denied": true }))
    }

    /// The public decisions this node's own identity signed, as hex. The
    /// built-in moderator's are left out, as with its rulings.
    pub(crate) fn dispute_decisions_value(&self) -> Vec<Value> {
        let desk = self.moderator_desk();
        let own = |id: &EscrowId| desk.cases.iter().any(|((case_id, _), case)| case_id == id && case.record.is_some());
        let recusals = desk
            .fees
            .iter()
            .filter(|(id, _)| own(id))
            .filter_map(|(_, fee)| fee.recusal.as_deref());
        desk.cases
            .values()
            .filter(|case| case.record.is_some())
            .filter_map(|case| case.decision.as_deref())
            .chain(recusals)
            .take(crate::moderators::MAX_RULINGS)
            .map(|decision| json!(hex(decision)))
            .collect()
    }
}
