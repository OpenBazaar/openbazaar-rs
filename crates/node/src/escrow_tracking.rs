//! A moderator's first-hand shipment lookup for a dispute.
//!
//! A dispute memo is free text: a vendor who wants its shipment considered
//! names the carrier and tracking number there, and a buyer may name the
//! ship-to locality. Neither claim is taken at its word. The operator copies
//! them into `POST /moderator/disputes/{id}/tracking`, and this node asks
//! the tracking provider itself, with the same bounded, coarse, fail-closed
//! lookup the seller's node uses ([`crate::tracking`]): only through Tor,
//! only carrier and tracking number sent, street addresses and names
//! discarded. What comes back is evidence the moderator observed, not
//! evidence a party relayed, so a fabricated timeline cannot survive it.
//!
//! When the operator also enters the expected ship-to locality, the
//! delivered checkpoint's coarse place is compared with it. The verdict is
//! advisory: carriers abbreviate and misreport places, and no checkpoint
//! shows what the parcel held, so the verdict informs the operator's ruling
//! and decides nothing itself. Looking up needs no moderator secret, so it
//! works while the wallet is locked, like reviewing a letter.

use serde_json::{json, Value};

use crate::escrow_mail::parse_id;
use crate::pay_order::PayError;
use crate::tracking::{bounded_text, carrier_slug, tracking_key, TrackingEvent};
use crate::Node;

/// The coarse locality the operator expects the parcel was delivered to,
/// copied from the buyer's letter. The same granularity as a stored
/// checkpoint: at most a city, state, and country.
struct ShipTo {
    city: String,
    state: String,
    country: String,
}

impl ShipTo {
    fn from_body(value: &Value) -> Result<Option<Self>, String> {
        let value = match value {
            Value::Null => return Ok(None),
            found => found,
        };
        if !value.is_object() {
            return Err("\"shipTo\" names a city, and possibly a state and country.".to_string());
        }
        let ship_to = Self {
            city: bounded_text(value["city"].as_str(), 60),
            state: bounded_text(value["state"].as_str(), 60),
            country: bounded_text(value["country"].as_str(), 60),
        };
        if normalized(&ship_to.city).is_empty() {
            return Err("The expected ship-to needs at least a city.".to_string());
        }
        Ok(Some(ship_to))
    }

    fn to_json(&self) -> Value {
        json!({ "city": self.city, "state": self.state, "country": self.country })
    }
}

/// A place name reduced for comparison: carriers and operators differ in
/// case, spacing, and punctuation, never meaningfully in letters and digits.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Equal when both sides name the place; no verdict from a side that is
/// silent. State and country are often left off checkpoints.
fn differs(expected: &str, found: &str) -> bool {
    let (expected, found) = (normalized(expected), normalized(found));
    !expected.is_empty() && !found.is_empty() && expected != found
}

/// A state reduced like [`normalized`], with a US state's full name reduced
/// to its postal abbreviation: buyers write "Oregon" where carriers write
/// "OR", and that difference is not a mismatch, while "Portland, ME"
/// against "Portland, OR" is.
fn normalized_state(text: &str) -> String {
    const US_STATES: &[(&str, &str)] = &[
        ("alabama", "al"), ("alaska", "ak"), ("arizona", "az"), ("arkansas", "ar"),
        ("california", "ca"), ("colorado", "co"), ("connecticut", "ct"), ("delaware", "de"),
        ("districtofcolumbia", "dc"), ("florida", "fl"), ("georgia", "ga"), ("hawaii", "hi"),
        ("idaho", "id"), ("illinois", "il"), ("indiana", "in"), ("iowa", "ia"),
        ("kansas", "ks"), ("kentucky", "ky"), ("louisiana", "la"), ("maine", "me"),
        ("maryland", "md"), ("massachusetts", "ma"), ("michigan", "mi"), ("minnesota", "mn"),
        ("mississippi", "ms"), ("missouri", "mo"), ("montana", "mt"), ("nebraska", "ne"),
        ("nevada", "nv"), ("newhampshire", "nh"), ("newjersey", "nj"), ("newmexico", "nm"),
        ("newyork", "ny"), ("northcarolina", "nc"), ("northdakota", "nd"), ("ohio", "oh"),
        ("oklahoma", "ok"), ("oregon", "or"), ("pennsylvania", "pa"), ("puertorico", "pr"),
        ("rhodeisland", "ri"), ("southcarolina", "sc"), ("southdakota", "sd"), ("tennessee", "tn"),
        ("texas", "tx"), ("utah", "ut"), ("vermont", "vt"), ("virginia", "va"),
        ("washington", "wa"), ("westvirginia", "wv"), ("wisconsin", "wi"), ("wyoming", "wy"),
    ];
    let reduced = normalized(text);
    US_STATES
        .iter()
        .find(|(full, _)| *full == reduced)
        .map_or(reduced, |(_, short)| (*short).to_string())
}

fn state_differs(expected: &str, found: &str) -> bool {
    let (expected, found) = (normalized_state(expected), normalized_state(found));
    !expected.is_empty() && !found.is_empty() && expected != found
}

/// The advisory verdict on where the carrier says the parcel ended up.
fn location_check(ship_to: &ShipTo, status: &str, events: &[TrackingEvent]) -> Value {
    let delivered = events.iter().rev().find(|event| event.status == "delivered");
    let (verdict, reason, place) = match delivered {
        None if status == "delivered" => (
            "unverifiable",
            "The carrier reports delivery but no delivered checkpoint.",
            None,
        ),
        None => ("unverifiable", "The carrier has not reported this shipment delivered.", None),
        Some(event) if normalized(&event.city).is_empty() => {
            ("unverifiable", "The delivered checkpoint names no city.", Some(event))
        }
        Some(event) => {
            if differs(&ship_to.city, &event.city)
                || state_differs(&ship_to.state, &event.state)
                || differs(&ship_to.country, &event.country)
            {
                ("mismatch", "The delivered checkpoint names a different locality.", Some(event))
            } else {
                ("match", "The delivered checkpoint names the expected locality.", Some(event))
            }
        }
    };
    json!({
        "verdict": verdict,
        "reason": reason,
        "deliveredCity": place.map_or("", |event| event.city.as_str()),
        "deliveredState": place.map_or("", |event| event.state.as_str()),
        "deliveredCountry": place.map_or("", |event| event.country.as_str()),
    })
}

impl Node {
    /// Look a disputed shipment up first-hand and record the answer on the
    /// case. `body`: `{"carrier", "trackingNumber", "shipTo"?}`, the first
    /// two copied from the dispute memo, the ship-to optional.
    pub fn dispute_tracking_lookup(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let escrow_id = parse_id(id).ok_or(PayError::NotFound)?;
        if !self.dispute_case_known(&escrow_id) {
            return Err(PayError::NotFound);
        }
        let carrier = body["carrier"].as_str().unwrap_or("");
        let Some(slug) = carrier_slug(carrier) else {
            return Err(PayError::Invalid(
                "Live lookup supports USPS, UPS, FedEx, and DHL.".to_string(),
            ));
        };
        let number = tracking_key(body["trackingNumber"].as_str().unwrap_or(""));
        if !(6..=64).contains(&number.len()) {
            return Err(PayError::Invalid("That tracking number cannot be looked up.".to_string()));
        }
        let ship_to = ShipTo::from_body(&body["shipTo"]).map_err(PayError::Invalid)?;
        let Some(provider) = self.tracking_provider() else {
            return Err(PayError::Invalid(
                "This node has no tracking provider. Start it with --tracking aftership, OB_TRACKING_API_KEY, and --proxy.".to_string(),
            ));
        };
        let snapshot = provider.fetch(slug, &number).map_err(|message| {
            // Provider errors are already reduced to safe categories; bodies
            // and headers never reach them ([`crate::tracking`]).
            if message.contains("--proxy") || message.contains("not supported") {
                PayError::Invalid(message)
            } else {
                PayError::Chain(message)
            }
        })?;
        let mut looked_up = snapshot.to_json(provider.name(), crate::escrow_outbox::now_secs());
        looked_up["carrier"] = json!(slug);
        looked_up["trackingNumber"] = json!(number);
        looked_up["shipTo"] = ship_to.as_ref().map_or(Value::Null, ShipTo::to_json);
        looked_up["locationCheck"] = ship_to.as_ref().map_or(Value::Null, |ship_to| {
            location_check(ship_to, &snapshot.status, &snapshot.events)
        });
        self.dispute_tracking_store(&escrow_id, &looked_up)
            .map_err(PayError::Failed)?;
        Ok(looked_up)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(status: &str, city: &str, state: &str, country: &str) -> TrackingEvent {
        TrackingEvent {
            at: "2026-11-13T15:04:05Z".to_string(),
            status: status.to_string(),
            city: city.to_string(),
            state: state.to_string(),
            country: country.to_string(),
        }
    }

    fn portland() -> ShipTo {
        ShipTo { city: "Portland".to_string(), state: "OR".to_string(), country: "United States".to_string() }
    }

    #[test]
    fn the_verdict_matches_coarse_places_despite_case_and_punctuation() {
        let delivered = [event("in_transit", "Memphis", "TN", ""), event("delivered", "PORTLAND", "or", "united-states")];
        let check = location_check(&portland(), "delivered", &delivered);
        assert_eq!(check["verdict"], "match", "{check}");
        assert_eq!(check["deliveredCity"], "PORTLAND");
    }

    #[test]
    fn a_us_state_written_out_matches_its_abbreviation_but_another_state_does_not() {
        let spelled = ShipTo { city: "Portland".to_string(), state: "Oregon".to_string(), country: String::new() };
        let delivered = [event("delivered", "Portland", "OR", "United States")];
        assert_eq!(location_check(&spelled, "delivered", &delivered)["verdict"], "match");
        let maine = [event("delivered", "Portland", "ME", "United States")];
        assert_eq!(location_check(&spelled, "delivered", &maine)["verdict"], "mismatch");
    }

    #[test]
    fn a_delivery_somewhere_else_is_a_mismatch() {
        let elsewhere = [event("delivered", "Boise", "ID", "United States")];
        let check = location_check(&portland(), "delivered", &elsewhere);
        assert_eq!(check["verdict"], "mismatch", "{check}");
        assert_eq!(check["reason"], "The delivered checkpoint names a different locality.");
    }

    #[test]
    fn a_checkpoint_silent_on_state_and_country_still_matches_on_city() {
        let coarse = [event("delivered", "Portland", "", "")];
        assert_eq!(location_check(&portland(), "delivered", &coarse)["verdict"], "match");
    }

    #[test]
    fn nothing_is_verified_before_delivery_or_without_a_place() {
        let moving = [event("in_transit", "Memphis", "TN", "")];
        let check = location_check(&portland(), "in_transit", &moving);
        assert_eq!(check["verdict"], "unverifiable");
        assert_eq!(check["reason"], "The carrier has not reported this shipment delivered.");
        let placeless = [event("delivered", "", "", "")];
        assert_eq!(location_check(&portland(), "delivered", &placeless)["verdict"], "unverifiable");
        assert_eq!(location_check(&portland(), "delivered", &[])["verdict"], "unverifiable");
    }

    #[test]
    fn the_expected_ship_to_needs_a_city_and_is_bounded() {
        assert!(ShipTo::from_body(&Value::Null).unwrap().is_none());
        assert!(ShipTo::from_body(&json!({ "state": "OR" })).is_err());
        assert!(ShipTo::from_body(&json!("Portland")).is_err());
        let long = ShipTo::from_body(&json!({ "city": "x".repeat(200) })).unwrap().unwrap();
        assert_eq!(long.city.len(), 60);
    }
}
