//! The filing-fee half of a moderator's track record, counted from the
//! records its ruling key signed: the cases it was paid for (`OBCA`), the
//! ones it decided or denied (`OBDD`), and the paid ones still undecided past
//! their due height. Nothing refunds an overdue fee; the count is the
//! public answer.

use std::collections::{BTreeMap, BTreeSet};

use openbazaar_protocol::{CaseAccepted, DisputeDecision, Signed};
use serde_json::{json, Value};

use crate::escrow_mail::unhex;
use crate::moderators::MAX_RULINGS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaseCounts {
    pub accepted: u32,
    pub decided: u32,
    pub denied: u32,
    pub overdue: u32,
}

impl CaseCounts {
    pub fn to_value(self) -> Value {
        json!({ "accepted": self.accepted, "decided": self.decided, "denied": self.denied, "overdue": self.overdue })
    }

    pub fn from_value(value: &Value) -> Self {
        let count = |key: &str| value[key].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        Self { accepted: count("accepted"), decided: count("decided"), denied: count("denied"), overdue: count("overdue") }
    }
}

fn signed_by<T: openbazaar_protocol::Terms>(moderator: &[u8; 32], list: &[Value]) -> Vec<T> {
    list.iter()
        .take(MAX_RULINGS)
        .filter_map(|entry| Signed::<T>::decode(&unhex(entry.as_str()?).ok()?).ok())
        .filter(|signed| signed.terms.signer() == *moderator && signed.verify().is_ok())
        .map(|signed| signed.terms)
        .collect()
}

/// Count the cases in `accepted` and `decisions` that `moderator` signed,
/// by case id. A paid case is overdue once `tip` is past its due height
/// with no decision.
pub(crate) fn count_cases(moderator: &[u8; 32], accepted: &[Value], decisions: &[Value], tip: u32) -> CaseCounts {
    let due: BTreeMap<[u8; 32], u32> = signed_by::<CaseAccepted>(moderator, accepted)
        .into_iter()
        .map(|case| (case.case_id, case.due_height))
        .collect();
    let decisions = signed_by::<DisputeDecision>(moderator, decisions);
    let decided: BTreeSet<[u8; 32]> = decisions.iter().map(|decision| decision.case_id).collect();
    let denied: BTreeSet<[u8; 32]> = decisions.iter().filter(|decision| decision.is_denial()).map(|decision| decision.case_id).collect();
    let overdue = due.iter().filter(|(id, due)| tip > **due && !decided.contains(*id)).count();
    CaseCounts {
        accepted: due.len() as u32,
        decided: decided.len() as u32,
        denied: denied.len() as u32,
        overdue: overdue as u32,
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::secp256k1::{Keypair, Secp256k1};
    use openbazaar_protocol::{DECISION_BUYER, DECISION_DENIED_VENDOR};
    use super::*;
    use crate::escrow_mail::hex;

    fn key(seed: u8) -> (Keypair, [u8; 32]) {
        let keypair = Keypair::from_seckey_slice(&Secp256k1::new(), &[seed; 32]).unwrap();
        let id = keypair.x_only_public_key().0.serialize();
        (keypair, id)
    }

    fn accepted(keys: &Keypair, case: u8, due: u32) -> Value {
        let terms = CaseAccepted { moderator_key: keys.x_only_public_key().0.serialize(), case_id: [case; 32], accepted_height: 10, due_height: due };
        json!(hex(&Signed::sign(terms, &keys.secret_key()).unwrap().encode().unwrap()))
    }

    fn decided(keys: &Keypair, case: u8, decision: u8) -> Value {
        let ruling_digest = if decision == DECISION_BUYER { [7; 32] } else { [0; 32] };
        let terms = DisputeDecision { moderator_key: keys.x_only_public_key().0.serialize(), case_id: [case; 32], decision, ruling_digest };
        json!(hex(&Signed::sign(terms, &keys.secret_key()).unwrap().encode().unwrap()))
    }

    #[test]
    fn counts_only_what_the_moderator_signed_and_flags_paid_cases_past_due() {
        let (keys, id) = key(3);
        let (other, _) = key(4);
        let accepted = [accepted(&keys, 1, 100), accepted(&keys, 2, 100), accepted(&keys, 3, 200), accepted(&other, 4, 100), json!("zz")];
        let decisions = [decided(&keys, 1, DECISION_BUYER), decided(&keys, 5, DECISION_DENIED_VENDOR), decided(&other, 2, DECISION_BUYER)];
        let counts = count_cases(&id, &accepted, &decisions, 150);
        assert_eq!(counts, CaseCounts { accepted: 3, decided: 2, denied: 1, overdue: 1 });
        assert_eq!(count_cases(&id, &accepted, &decisions, 100).overdue, 0, "due at 100 is not overdue at 100");
        assert_eq!(CaseCounts::from_value(&counts.to_value()), counts);
        assert_eq!(CaseCounts::from_value(&json!(null)), CaseCounts::default());
    }
}
