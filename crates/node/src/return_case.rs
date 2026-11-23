//! A return case on a sealed order.
//!
//! The listing or store already carries a return policy as text. This turns
//! that policy into a case the two parties of the order can move, as a signed
//! order message on the same channel as chat and shipment notices. It records
//! the request, the seller's accept or decline, and an optional return
//! shipment (carrier, tracking number, note). It does not pay anyone, release
//! escrow, or ask a tracking provider.
//!
//! A return is in one of [`STATE_REQUESTED`], [`STATE_ACCEPTED`],
//! [`STATE_DECLINED`], or [`STATE_SHIPPED`]. A message that is not one of
//! those steps is ignored. Applying one never changes the order's status,
//! payment, or anything outside `returnCase`.

use openbazaar_protocol::{plain_text, public_text, Returns};
use serde_json::{json, Value};

use crate::messages::MAX_TEXT;
use crate::watch::{COMPLETED, SHIPPED};
use crate::Node;

/// The buyer has asked to return the order.
pub(crate) const STATE_REQUESTED: &str = "requested";
/// The seller will take the goods back. No money moves here.
pub(crate) const STATE_ACCEPTED: &str = "accepted";
/// The seller declined the return.
pub(crate) const STATE_DECLINED: &str = "declined";
/// The buyer named a return shipment. The order itself stays where it was.
pub(crate) const STATE_SHIPPED: &str = "shipped";

/// A store returns policy is at most this many bytes.
const MAX_POLICY: usize = 1_000;
const CARRIER_MAX: usize = 60;
const TRACKING_MAX: usize = 100;
const NOTE_MAX: usize = 1_000;

pub(crate) struct Shipment {
    pub carrier: String,
    pub tracking: String,
    pub note: String,
}

impl Node {
    /// The return policy these listing terms offered, frozen onto the sale.
    pub(crate) fn return_policy_of(&self, returns: &[Returns]) -> Option<String> {
        let store = self
            .own_record()
            .and_then(|store| store.details)
            .map(|details| details.profile.returns_policy)
            .unwrap_or_default();
        policy_text(returns, &store)
    }
}

/// The first line that actually allows a return. `NoReturns` opts that line
/// out even when the store has a policy. A store policy is used only for a
/// line that says to follow it, and only when the text itself is plain.
pub(crate) fn policy_text(returns: &[Returns], store_policy: &str) -> Option<String> {
    returns.iter().find_map(|policy| match *policy {
        Returns::NoReturns => None,
        Returns::Days { days, .. } if days == 0 || days > 365 => None,
        Returns::Days { days, seller_pays } => Some(days_line(days, seller_pays)),
        Returns::Store => usable_policy(store_policy),
    })
}

/// A policy string from a seller's terms reply. Anything that is not short
/// plain text is dropped, and the rest of the reply still applies.
pub(crate) fn policy_from_reply(value: &Value) -> Option<String> {
    usable_policy(value.as_str()?)
}

fn days_line(days: u16, seller_pays: bool) -> String {
    let payer = if seller_pays {
        "The seller"
    } else {
        "The buyer"
    };
    format!("Returns within {days} days. {payer} pays return shipping.")
}

fn usable_policy(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_POLICY || !plain_text::is_plain(text, true) {
        return None;
    }
    public_text::find(text).is_none().then(|| text.to_string())
}

/// The buyer's reason, bounded like an order message.
pub(crate) fn request_text(body: &Value) -> Result<String, String> {
    let raw = body["text"]
        .as_str()
        .ok_or_else(|| "Say why you are returning it.".to_string())?;
    prose(raw, MAX_TEXT, "Say why you are returning it.")
}

/// `accept` or `decline`, with an optional note.
pub(crate) fn decision_from(body: &Value) -> Result<(bool, String), String> {
    let accept = match body["decision"].as_str() {
        Some("accept") => true,
        Some("decline") => false,
        _ => return Err("Accept or decline the return.".to_string()),
    };
    Ok((accept, optional_prose(body, "text", MAX_TEXT)?))
}

/// Carrier and tracking, with the same bounds as an outbound shipment.
/// The tracking number is not run through the phone check: a carrier number
/// is a run of digits on purpose.
pub(crate) fn shipment_from(body: &Value) -> Result<Shipment, String> {
    let carrier = short_field(body, "carrier", CARRIER_MAX)?;
    let tracking = short_field(body, "tracking", TRACKING_MAX)?;
    let note = optional_prose(body, "note", NOTE_MAX)?;
    if carrier.is_empty() && tracking.is_empty() {
        return Err("Add the carrier or a tracking number.".to_string());
    }
    Ok(Shipment {
        carrier,
        tracking,
        note,
    })
}

fn short_field(body: &Value, key: &str, max: usize) -> Result<String, String> {
    let raw = match body.get(key) {
        None | Some(Value::Null) => "",
        Some(Value::String(text)) => text.as_str(),
        Some(_) => return Err(format!("The {key} could not be read.")),
    };
    let text = raw.trim();
    if text.chars().count() > max || !plain_text::is_plain(text, false) {
        return Err(format!("The {key} is too long."));
    }
    Ok(text.to_string())
}

fn optional_prose(body: &Value, key: &str, max: usize) -> Result<String, String> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) if text.trim().is_empty() => Ok(String::new()),
        Some(Value::String(text)) => prose(text, max, "That note could not be read."),
        Some(_) => Err("That note could not be read.".to_string()),
    }
}

fn prose(raw: &str, max_chars: usize, empty: &str) -> Result<String, String> {
    let text: String = raw
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .collect();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(empty.to_string());
    }
    if text.chars().count() > max_chars {
        return Err(format!("Keep this under {max_chars} characters."));
    }
    if !plain_text::is_plain(&text, true) {
        return Err("That text has characters this order cannot store.".to_string());
    }
    if let Some(kind) = public_text::find(&text) {
        return Err(format!("Take out {}.", kind.as_str()));
    }
    Ok(text)
}

/// `None` when there is no case. `Some("")` when a case is present but its
/// state is not one this node wrote, so a later message cannot overwrite it.
fn case_state(owner: &Value) -> Option<&str> {
    let case = owner.get("returnCase")?;
    if case.is_null() {
        return None;
    }
    Some(case.get("state").and_then(Value::as_str).unwrap_or(""))
}

/// Record the buyer's request. The sale must already carry a return policy
/// and have shipped. Writes only `returnCase`.
pub(crate) fn buyer_request(sale: &mut Value, reason: &str, now: u64) -> Result<(), &'static str> {
    if sale["returnPolicy"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return Err("This order has no return policy.");
    }
    let status = sale["status"].as_str().unwrap_or("");
    if status != SHIPPED && status != COMPLETED {
        return Err("A return opens once the order has shipped.");
    }
    open_request(sale, reason, now)
}

/// Record a request the buyer already checked against the policy and status.
pub(crate) fn record_request(
    owner: &mut Value,
    reason: &str,
    now: u64,
) -> Result<(), &'static str> {
    open_request(owner, reason, now)
}

fn open_request(owner: &mut Value, reason: &str, now: u64) -> Result<(), &'static str> {
    match case_state(owner) {
        None => {
            owner["returnCase"] = json!({
                "state": STATE_REQUESTED,
                "reason": reason,
                "requestedAt": now,
            });
            Ok(())
        }
        Some(STATE_REQUESTED) if owner["returnCase"]["reason"].as_str() == Some(reason) => Ok(()),
        Some(_) => Err("This order already has a return."),
    }
}

/// The seller accepts or declines. A repeat of the same decision is a no-op.
pub(crate) fn decide(
    owner: &mut Value,
    accept: bool,
    note: &str,
    now: u64,
) -> Result<(), &'static str> {
    let next = if accept {
        STATE_ACCEPTED
    } else {
        STATE_DECLINED
    };
    match case_state(owner) {
        Some(state)
            if state == next && owner["returnCase"]["decision"].as_str().unwrap_or("") == note =>
        {
            Ok(())
        }
        Some(STATE_REQUESTED) => {
            let mut case = owner["returnCase"].clone();
            case["state"] = json!(next);
            case["decision"] = json!(note);
            case["decidedAt"] = json!(now);
            owner["returnCase"] = case;
            Ok(())
        }
        Some(_) => Err("This return is no longer waiting for a decision."),
        None => Err("There is no return to decide."),
    }
}

/// The buyer names the return shipment after the seller has accepted.
pub(crate) fn mark_shipped(
    owner: &mut Value,
    shipment: &Shipment,
    now: u64,
) -> Result<(), &'static str> {
    let same = owner["returnCase"]["shipment"]["carrier"].as_str()
        == Some(shipment.carrier.as_str())
        && owner["returnCase"]["shipment"]["tracking"].as_str() == Some(shipment.tracking.as_str())
        && owner["returnCase"]["shipment"]["note"]
            .as_str()
            .unwrap_or("")
            == shipment.note;
    match case_state(owner) {
        Some(STATE_SHIPPED) if same => Ok(()),
        Some(STATE_ACCEPTED) => {
            let mut case = owner["returnCase"].clone();
            case["state"] = json!(STATE_SHIPPED);
            case["shipment"] = json!({
                "carrier": shipment.carrier,
                "tracking": shipment.tracking,
                "note": shipment.note,
                "at": now,
            });
            owner["returnCase"] = case;
            Ok(())
        }
        Some(_) => Err("A return shipment follows an accepted return."),
        None => Err("There is no return to ship."),
    }
}

/// Apply a seller's return reply. An unknown or illegal reply leaves `owner`
/// as it was, so one bad letter cannot move the order.
pub(crate) fn apply_seller_reply(owner: &mut Value, body: &Value, now: u64) -> Result<(), ()> {
    let action = body["action"].as_str().ok_or(())?;
    let note = match body.get("text") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) if text.trim().is_empty() => String::new(),
        Some(Value::String(text)) => {
            prose(text, MAX_TEXT, "That note could not be read.").map_err(|_| ())?
        }
        Some(_) => return Err(()),
    };
    let accept = match action {
        "accept" => true,
        "decline" => false,
        _ => return Err(()),
    };
    decide(owner, accept, &note, now).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_sale(policy: &str) -> Value {
        json!({
            "status": SHIPPED,
            "totalSats": 28000,
            "returnPolicy": policy,
            "payment": { "address": "bcrt1qexample" }
        })
    }

    #[test]
    fn a_policy_string_or_a_window_allows_a_return_and_no_returns_does_not() {
        let store = "Unused goods can come back within 30 days.";
        assert_eq!(
            policy_text(&[Returns::Store], store).as_deref(),
            Some(store)
        );
        assert_eq!(policy_text(&[Returns::NoReturns], store), None);
        assert_eq!(policy_text(&[Returns::Store], "  "), None);
        assert_eq!(
            policy_text(
                &[Returns::Days {
                    days: 14,
                    seller_pays: true
                }],
                store
            )
            .as_deref(),
            Some("Returns within 14 days. The seller pays return shipping.")
        );
        assert_eq!(
            policy_text(
                &[Returns::Days {
                    days: 0,
                    seller_pays: false
                }],
                store
            ),
            None
        );
        assert!(policy_from_reply(&json!("a@b.co")).is_none());
        assert!(policy_from_reply(&json!("ok\u{202e}exe")).is_none());
        assert_eq!(policy_from_reply(&json!(store)).as_deref(), Some(store));
    }

    #[test]
    fn request_accept_and_ship_move_only_the_case() {
        let mut sale = shipped_sale("Unused goods can come back within 30 days.");
        let before = sale.clone();
        buyer_request(&mut sale, "The glaze is crazed.", 10).unwrap();
        assert_eq!(sale["returnCase"]["state"], STATE_REQUESTED);
        assert_eq!(sale["status"], before["status"]);
        assert_eq!(sale["totalSats"], before["totalSats"]);
        assert_eq!(sale["payment"], before["payment"]);
        buyer_request(&mut sale, "The glaze is crazed.", 11).unwrap();
        assert!(buyer_request(&mut sale, "A different story.", 12).is_err());
        assert_eq!(sale["returnCase"]["reason"], "The glaze is crazed.");

        decide(&mut sale, true, "Send it back.", 20).unwrap();
        assert_eq!(sale["returnCase"]["state"], STATE_ACCEPTED);
        assert_eq!(sale["status"], SHIPPED);
        let shipment = Shipment {
            carrier: "USPS".into(),
            tracking: "9400111100000000000000".into(),
            note: String::new(),
        };
        mark_shipped(&mut sale, &shipment, 30).unwrap();
        assert_eq!(sale["returnCase"]["state"], STATE_SHIPPED);
        assert_eq!(
            sale["returnCase"]["shipment"]["tracking"],
            "9400111100000000000000"
        );
        assert_eq!(sale["status"], SHIPPED);
        assert_eq!(sale["payment"]["address"], "bcrt1qexample");
        mark_shipped(&mut sale, &shipment, 31).unwrap();
    }

    #[test]
    fn a_decline_ends_the_case_and_a_bad_reply_changes_nothing() {
        let mut sale = shipped_sale("Returns within 14 days.");
        assert!(buyer_request(&mut sale, "Arrived broken.", 1).is_ok());
        decide(&mut sale, false, "", 2).unwrap();
        assert_eq!(sale["returnCase"]["state"], STATE_DECLINED);
        assert!(decide(&mut sale, true, "", 3).is_err());
        assert!(mark_shipped(
            &mut sale,
            &Shipment {
                carrier: "USPS".into(),
                tracking: "1".into(),
                note: String::new()
            },
            4
        )
        .is_err());
        assert_eq!(sale["returnCase"]["state"], STATE_DECLINED);
        assert_eq!(sale["status"], SHIPPED);

        let snapshot = sale.clone();
        assert!(apply_seller_reply(
            &mut sale,
            &json!({ "action": "release", "text": "pay the buyer" }),
            5
        )
        .is_err());
        assert!(
            apply_seller_reply(&mut sale, &json!({ "action": "accept", "text": 12 }), 5).is_err()
        );
        assert!(apply_seller_reply(
            &mut sale,
            &json!({ "action": "accept", "text": "write a@b.co" }),
            5
        )
        .is_err());
        assert_eq!(sale, snapshot);
    }

    #[test]
    fn an_unshipped_order_and_a_missing_policy_cannot_open_a_case() {
        let mut unpaid = json!({ "status": "Paid", "returnPolicy": "30 days" });
        assert!(buyer_request(&mut unpaid, "Too late.", 1).is_err());
        assert!(unpaid.get("returnCase").is_none());
        let mut final_sale = json!({ "status": SHIPPED });
        assert!(buyer_request(&mut final_sale, "Please.", 1).is_err());
        assert!(final_sale.get("returnCase").is_none());
    }

    #[test]
    fn reasons_are_bounded_and_tracking_numbers_are_allowed() {
        assert!(request_text(&json!({ "text": "  hi\u{7} there " })).unwrap() == "hi there");
        assert!(request_text(&json!({ "text": "   " })).is_err());
        assert!(request_text(&json!({ "text": "x".repeat(MAX_TEXT + 1) })).is_err());
        assert!(request_text(&json!({ "text": "see a@b.co" })).is_err());
        assert!(request_text(&json!({ "text": "ok\u{202e}exe" })).is_err());
        let body =
            json!({ "carrier": "USPS", "tracking": "9400 1111 0000 0000 0000 00", "note": "" });
        let shipment = shipment_from(&body).unwrap();
        assert_eq!(shipment.tracking, "9400 1111 0000 0000 0000 00");
        assert!(shipment_from(&json!({ "carrier": "", "tracking": "" })).is_err());
        assert!(
            shipment_from(&json!({ "carrier": "USPS\u{202e}", "tracking": "123456" })).is_err()
        );
        assert_eq!(
            decision_from(&json!({ "decision": "accept" })).unwrap(),
            (true, String::new())
        );
        assert!(decision_from(&json!({ "decision": "refund" })).is_err());
    }
}
