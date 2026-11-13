//! Private shipment tracking for physical sales.
//!
//! Only the seller's node talks to a tracking provider, and only through
//! the node's Tor proxy. The provider is told a carrier and tracking number
//! and nothing else: no order id, buyer, address, item, or amount. The
//! buyer's node never contacts the provider. It receives normalized updates
//! in the seller's existing sender-pinned, sealed replies.
//!
//! Provider checkpoints can contain street addresses, coordinates, and the
//! name of a person who signed. Those fields are deliberately discarded.
//! What is stored or sent to the buyer is a coarse status, a time, and at
//! most a city, state, and country.

use std::io::Read;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::egress::Egress;
use crate::watch::SHIPPED;
use crate::Node;

const AFTERSHIP_BASE: &str = "https://api.aftership.com/tracking/2026-07";
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;
const MAX_EVENTS: usize = 50;
const CHECK_EVERY_SECS: u64 = 6 * 60 * 60;
const RETRY_EVERY_SECS: u64 = 60 * 60;
const MAX_CHECKS_PER_ROUND: usize = 3;

const STATUSES: &[&str] = &[
    "pending",
    "label_created",
    "in_transit",
    "out_for_delivery",
    "delivery_attempted",
    "available_for_pickup",
    "delivered",
    "exception",
    "returned",
    "expired",
    "unknown",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackingEvent {
    pub(crate) at: String,
    pub(crate) status: String,
    pub(crate) city: String,
    pub(crate) state: String,
    pub(crate) country: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackingSnapshot {
    pub(crate) status: String,
    pub(crate) events: Vec<TrackingEvent>,
    pub(crate) delivered_at: Option<String>,
}

impl TrackingSnapshot {
    fn to_json(&self, provider: &str, checked_at: u64) -> Value {
        json!({
            "provider": provider,
            "status": self.status,
            "label": status_label(&self.status),
            "events": self.events.iter().map(|event| json!({
                "at": event.at,
                "status": event.status,
                "label": status_label(&event.status),
                "city": event.city,
                "state": event.state,
                "country": event.country,
            })).collect::<Vec<_>>(),
            "deliveredAt": self.delivered_at,
            "checkedAt": checked_at,
            "updatedAt": checked_at,
        })
    }

    fn same_progress(&self, stored: &Value) -> bool {
        stored["status"] == self.status.as_str()
            && stored["deliveredAt"]
                == self
                    .delivered_at
                    .as_deref()
                    .map_or(Value::Null, |value| json!(value))
            && stored["events"].as_array().is_some_and(|events| {
                events.len() == self.events.len()
                    && events.iter().zip(&self.events).all(|(stored, event)| {
                        stored["at"] == event.at.as_str()
                            && stored["status"] == event.status.as_str()
                            && stored["city"] == event.city.as_str()
                            && stored["state"] == event.state.as_str()
                            && stored["country"] == event.country.as_str()
                    })
            })
    }
}

/// A tracking lookup. Implementations must not send order or party data.
pub(crate) trait TrackingProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch(&self, carrier_slug: &str, tracking_number: &str) -> Result<TrackingSnapshot, String>;
}

/// AfterShip lookup by carrier slug and tracking number.
///
/// The API key lives only in this value. It is never stored with a sale,
/// sent to a buyer, returned by an HTTP route, or written to a log.
pub(crate) struct AfterShip {
    egress: Egress,
    api_key: String,
}

impl AfterShip {
    pub(crate) fn new(egress: Egress, api_key: String) -> Result<Self, String> {
        let api_key = api_key.trim().to_string();
        if api_key.is_empty() || api_key.len() > 512 || api_key.chars().any(char::is_control) {
            return Err("OB_TRACKING_API_KEY is empty or unreadable".to_string());
        }
        Ok(Self { egress, api_key })
    }

    fn request(&self, request: ureq::Request) -> Result<Value, AfterShipError> {
        self.finish(
            request
                .set("as-api-key", &self.api_key)
                .set("Accept", "application/json")
                .set("User-Agent", "OpenBazaar")
                .call(),
        )
    }

    fn request_body(&self, request: ureq::Request, body: &str) -> Result<Value, AfterShipError> {
        self.finish(
            request
                .set("as-api-key", &self.api_key)
                .set("Accept", "application/json")
                .set("User-Agent", "OpenBazaar")
                .set("Content-Type", "application/json")
                .send_string(body),
        )
    }

    fn finish(&self, call: Result<ureq::Response, ureq::Error>) -> Result<Value, AfterShipError> {
        let response = call.map_err(|err| match err {
            ureq::Error::Status(404, _) => AfterShipError::NotFound,
            ureq::Error::Status(status, _) => {
                AfterShipError::Refused(format!("tracking provider answered HTTP {status}"))
            }
            ureq::Error::Transport(_) => AfterShipError::Refused(
                "tracking provider could not be reached through the proxy".to_string(),
            ),
        })?;
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| {
                AfterShipError::Refused("tracking provider sent an unreadable answer".to_string())
            })?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(AfterShipError::Refused(
                "tracking provider sent too large an answer".to_string(),
            ));
        }
        serde_json::from_slice(&body).map_err(|_| {
            AfterShipError::Refused("tracking provider sent an unreadable answer".to_string())
        })
    }
}

enum AfterShipError {
    NotFound,
    Refused(String),
}

impl TrackingProvider for AfterShip {
    fn name(&self) -> &'static str {
        "aftership"
    }

    fn fetch(&self, slug: &str, tracking_number: &str) -> Result<TrackingSnapshot, String> {
        if self.egress.proxy.is_none() {
            return Err("tracking provider is not contacted without --proxy".to_string());
        }
        if carrier_slug(slug).is_none_or(|known| known != slug) {
            return Err("tracking carrier is not supported".to_string());
        }
        let tracking_number = tracking_key(tracking_number);
        if !(6..=64).contains(&tracking_number.len()) {
            return Err("tracking number cannot be looked up".to_string());
        }
        let agent = self
            .egress
            .ureq_agent(
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(10),
            )
            .map_err(|_| "tracking proxy could not be configured".to_string())?;
        let url =
            format!("{AFTERSHIP_BASE}/trackings/{slug}/{tracking_number}?fields=tag,checkpoints");
        let fetched = self.request(agent.get(&url));
        let body = match fetched {
            Ok(body) => body,
            Err(AfterShipError::NotFound) => {
                // The only data sent to create the tracker: carrier + number.
                let created = agent.post(&format!("{AFTERSHIP_BASE}/trackings"));
                let body =
                    json!({ "tracking": { "tracking_number": tracking_number, "slug": slug } })
                        .to_string();
                self.request_body(created, &body).map_err(|err| match err {
                    AfterShipError::NotFound => {
                        "tracking provider could not create that tracker".to_string()
                    }
                    AfterShipError::Refused(message) => message,
                })?
            }
            Err(AfterShipError::Refused(message)) => return Err(message),
        };
        parse_aftership(&body, slug, &tracking_number)
    }
}

/// The carrier slug a free-form seller entry names, if live lookup supports it.
pub(crate) fn carrier_slug(carrier: &str) -> Option<&'static str> {
    let normalized: String = carrier
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match normalized.as_str() {
        "usps" | "unitedstatespostalservice" => Some("usps"),
        "ups" | "unitedparcelservice" => Some("ups"),
        "fedex" | "fedexexpress" | "federalexpress" => Some("fedex"),
        "dhl" | "dhlexpress" => Some("dhl"),
        _ => None,
    }
}

/// A tracking number reduced to the characters carriers put in lookup URLs.
pub(crate) fn tracking_key(tracking: &str) -> String {
    tracking
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

pub(crate) fn initial_tracking(carrier: &str, tracking: &str, now: u64) -> Value {
    let supported =
        carrier_slug(carrier).is_some() && (6..=64).contains(&tracking_key(tracking).len());
    json!({
        "provider": supported.then_some("aftership"),
        "status": if supported { "pending" } else { "manual" },
        "events": [],
        "deliveredAt": Value::Null,
        "checkedAt": 0,
        "updatedAt": 0,
        "nextCheck": if supported { now } else { u64::MAX },
    })
}

fn status_label(status: &str) -> &'static str {
    match status {
        "pending" => "Waiting for carrier updates",
        "label_created" => "Label created",
        "in_transit" => "In transit",
        "out_for_delivery" => "Out for delivery",
        "delivery_attempted" => "Delivery attempted",
        "available_for_pickup" => "Available for pickup",
        "delivered" => "Delivered",
        "exception" => "Delivery exception",
        "returned" => "Returned to sender",
        "expired" => "Tracking expired",
        "manual" => "Tracking entered by the seller",
        _ => "Tracking update",
    }
}

fn bounded_text(value: Option<&str>, max: usize) -> String {
    value
        .unwrap_or("")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

fn timestamp(value: Option<&str>) -> Option<String> {
    let value = bounded_text(value, 40);
    if value.len() < 10
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'.' | b'+' | b'Z')
        })
    {
        return None;
    }
    Some(value)
}

fn normalize_status(value: &str) -> &'static str {
    match value {
        "Pending" | "pending" => "pending",
        "InfoReceived" | "label_created" => "label_created",
        "InTransit" | "in_transit" => "in_transit",
        "OutForDelivery" | "out_for_delivery" => "out_for_delivery",
        "AttemptFail" | "delivery_attempted" => "delivery_attempted",
        "AvailableForPickup" | "available_for_pickup" => "available_for_pickup",
        "Delivered" | "delivered" => "delivered",
        "Exception" | "exception" => "exception",
        "returned" => "returned",
        "Expired" | "expired" => "expired",
        _ => "unknown",
    }
}

fn parse_aftership(
    body: &Value,
    carrier_slug: &str,
    tracking_number: &str,
) -> Result<TrackingSnapshot, String> {
    let tracking = &body["data"]["tracking"];
    if !tracking.is_object() {
        return Err("tracking provider sent no tracking record".to_string());
    }
    let returned = tracking_key(tracking["tracking_number"].as_str().unwrap_or(""));
    if returned != tracking_number {
        return Err("tracking provider answered for a different tracking number".to_string());
    }
    if let Some(slug) = tracking["slug"].as_str().filter(|slug| !slug.is_empty()) {
        if slug != carrier_slug {
            return Err("tracking provider answered for a different carrier".to_string());
        }
    }
    let mut events = Vec::new();
    if let Some(checkpoints) = tracking["checkpoints"].as_array() {
        for checkpoint in checkpoints.iter().take(200) {
            let Some(at) = timestamp(checkpoint["checkpoint_time"].as_str()) else {
                continue;
            };
            // `location`, coordinates, `signed_by`, and proof-of-delivery
            // fields are intentionally never read.
            events.push(TrackingEvent {
                at,
                status: normalize_status(checkpoint["tag"].as_str().unwrap_or("")).to_string(),
                city: bounded_text(checkpoint["city"].as_str(), 60),
                state: bounded_text(checkpoint["state"].as_str(), 60),
                country: bounded_text(checkpoint["country_name"].as_str(), 60),
            });
        }
    }
    events.sort_by(|a, b| a.at.cmp(&b.at));
    events.dedup();
    if events.len() > MAX_EVENTS {
        events = events.split_off(events.len() - MAX_EVENTS);
    }
    let status = normalize_status(tracking["tag"].as_str().unwrap_or(""));
    let status = if status == "unknown" {
        events
            .last()
            .map_or("unknown", |event| normalize_status(&event.status))
    } else {
        status
    };
    let delivered_at = events
        .iter()
        .rev()
        .find(|event| event.status == "delivered")
        .map(|event| event.at.clone());
    Ok(TrackingSnapshot {
        status: status.to_string(),
        events,
        delivered_at,
    })
}

/// Validate and reduce a seller's tracking reply on the buyer's node.
///
/// The sender is already pinned to this order's seller, but the body is still
/// untrusted data: only known statuses and bounded coarse locations survive.
pub(crate) fn tracking_update_from_body(
    body: &Value,
    carrier: &str,
    tracking: &str,
    received_at: u64,
) -> Option<Value> {
    if carrier_slug(body["carrier"].as_str().unwrap_or("")) != carrier_slug(carrier)
        || tracking_key(body["trackingNumber"].as_str().unwrap_or("")) != tracking_key(tracking)
    {
        return None;
    }
    let update = &body["tracking"];
    let status = update["status"].as_str().unwrap_or("");
    if !STATUSES.contains(&status) {
        return None;
    }
    let mut events = Vec::new();
    for event in update["events"].as_array()?.iter().take(MAX_EVENTS) {
        let event_status = event["status"].as_str().unwrap_or("");
        if !STATUSES.contains(&event_status) {
            continue;
        }
        events.push(json!({
            "at": timestamp(event["at"].as_str())?,
            "status": event_status,
            "label": status_label(event_status),
            "city": bounded_text(event["city"].as_str(), 60),
            "state": bounded_text(event["state"].as_str(), 60),
            "country": bounded_text(event["country"].as_str(), 60),
        }));
    }
    Some(json!({
        "provider": "aftership",
        "status": status,
        "label": status_label(status),
        "events": events,
        "deliveredAt": timestamp(update["deliveredAt"].as_str()),
        "updatedAt": received_at,
    }))
}

impl Node {
    /// Enable AfterShip on this node. The key is kept only in memory.
    pub fn enable_tracking_aftership(&self, api_key: String) -> Result<(), String> {
        let provider = AfterShip::new(self.local.egress.clone(), api_key)?;
        *self.tracking.lock().unwrap_or_else(|err| err.into_inner()) = Some(Arc::new(provider));
        Ok(())
    }

    pub(crate) fn tracking_provider(&self) -> Option<Arc<dyn TrackingProvider>> {
        self.tracking
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn set_tracking_provider(&self, provider: Arc<dyn TrackingProvider>) {
        *self.tracking.lock().unwrap_or_else(|err| err.into_inner()) = Some(provider);
    }

    /// Poll due shipments on the seller's node and seal any progress to buyers.
    ///
    /// Called by the watcher. At most a few shipments are checked per round,
    /// each at most every six hours. Nothing is fetched without a provider,
    /// and the AfterShip provider itself refuses without `--proxy`.
    pub(crate) fn watch_tracking(&self) -> usize {
        let Some(provider) = self.tracking_provider() else {
            return 0;
        };
        let now = crate::escrow_outbox::now_secs();
        let candidates: Vec<(String, String, String)> = {
            let sales = self.sales();
            sales
                .list()
                .filter(|sale| {
                    sale["status"] == SHIPPED
                        && (sale.get("tracking").is_none()
                            || sale["tracking"]["nextCheck"]
                                .as_u64()
                                .is_some_and(|next| next <= now))
                })
                .filter_map(|sale| {
                    let fulfillment = &sale["fulfillment"];
                    let slug = carrier_slug(fulfillment["carrier"].as_str().unwrap_or(""))?;
                    let tracking = fulfillment["tracking"].as_str().unwrap_or("");
                    Some((
                        sale["id"].as_str()?.to_string(),
                        slug.to_string(),
                        tracking.to_string(),
                    ))
                })
                .take(MAX_CHECKS_PER_ROUND)
                .collect()
        };
        let mut changed = 0;
        for (id, slug, tracking_number) in candidates {
            match provider.fetch(&slug, &tracking_key(&tracking_number)) {
                Ok(snapshot) => {
                    let next = if matches!(
                        snapshot.status.as_str(),
                        "delivered" | "returned" | "expired"
                    ) {
                        u64::MAX
                    } else {
                        now.saturating_add(CHECK_EVERY_SECS)
                    };
                    let mut tell: Option<Value> = None;
                    let updated = self.sales().update(&id, |sale| {
                        let progressed = !snapshot.same_progress(&sale["tracking"]);
                        let mut value = snapshot.to_json(provider.name(), now);
                        value["nextCheck"] = json!(next);
                        sale["tracking"] = value;
                        if progressed {
                            tell = Some(sale.clone());
                        }
                    });
                    match updated {
                        Ok(Some(_)) => {
                            if let Some(sale) = tell {
                                changed += 1;
                                let reply = json!({
                                    "v": 1,
                                    "kind": "tracking",
                                    "carrier": sale["fulfillment"]["carrier"],
                                    "trackingNumber": sale["fulfillment"]["tracking"],
                                    "tracking": sale["tracking"],
                                });
                                if let Err(err) = self.tell_buyer(&sale, &reply) {
                                    eprintln!("ob-node: tracking update for sale {id}: {err}");
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(err) => eprintln!("ob-node: tracking update for sale {id}: {err}"),
                    }
                }
                Err(err) => {
                    // Only the category is kept. Provider bodies and headers
                    // are never stored or logged.
                    let safe = if err.contains("--proxy") {
                        err
                    } else {
                        "tracking lookup failed".to_string()
                    };
                    if let Err(err) = self.sales().update(&id, |sale| {
                        sale["tracking"]["nextCheck"] = json!(now.saturating_add(RETRY_EVERY_SECS));
                        sale["tracking"]["lookupError"] = json!(safe);
                    }) {
                        eprintln!("ob-node: tracking retry for sale {id}: {err}");
                    }
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriers_and_tracking_numbers_are_normalized() {
        assert_eq!(carrier_slug("USPS"), Some("usps"));
        assert_eq!(carrier_slug("FedEx Express"), Some("fedex"));
        assert_eq!(carrier_slug("DHL"), Some("dhl"));
        assert_eq!(carrier_slug("Local courier"), None);
        assert_eq!(tracking_key("9400 1000-00"), "9400100000");
    }

    #[test]
    fn aftership_answers_keep_only_coarse_checkpoint_places() {
        let body = json!({
            "data": { "tracking": {
                "tracking_number": "9400100000",
                "slug": "usps",
                "tag": "Delivered",
                "checkpoints": [{
                    "checkpoint_time": "2026-11-13T15:04:05Z",
                    "tag": "Delivered",
                    "city": "Portland",
                    "state": "OR",
                    "country_name": "United States",
                    "location": "14 Larch Lane, Portland, OR",
                    "coordinate": [45.5, -122.6],
                    "signed_by": "B. Buyer"
                }]
            }}
        });
        let snapshot = parse_aftership(&body, "usps", "9400100000").unwrap();
        assert_eq!(snapshot.status, "delivered");
        assert_eq!(
            snapshot.delivered_at.as_deref(),
            Some("2026-11-13T15:04:05Z")
        );
        let stored = snapshot.to_json("aftership", 10).to_string();
        assert!(stored.contains("Portland"));
        assert!(!stored.contains("Larch"));
        assert!(!stored.contains("45.5"));
        assert!(!stored.contains("B. Buyer"));
    }

    #[test]
    fn aftership_answers_for_another_parcel_are_refused() {
        let body = json!({ "data": { "tracking": { "tracking_number": "OTHER", "slug": "usps", "tag": "InTransit", "checkpoints": [] } } });
        assert!(parse_aftership(&body, "usps", "9400100000").is_err());
    }

    #[test]
    fn buyer_side_updates_must_match_the_shipment_and_known_statuses() {
        let body = json!({
            "carrier": "USPS",
            "trackingNumber": "9400 1000 00",
            "tracking": {
                "status": "in_transit",
                "events": [{ "at": "2026-11-13T15:04:05Z", "status": "in_transit", "city": "Portland", "state": "OR", "country": "United States", "location": "14 Larch Lane" }],
                "deliveredAt": null
            }
        });
        let update = tracking_update_from_body(&body, "USPS", "9400100000", 99).unwrap();
        assert_eq!(update["updatedAt"], 99);
        assert!(!update.to_string().contains("Larch"));
        assert!(tracking_update_from_body(&body, "UPS", "9400100000", 99).is_none());
    }

    #[test]
    fn aftership_never_calls_out_without_a_proxy() {
        let provider = AfterShip::new(Egress::default(), "test-key".to_string()).unwrap();
        assert!(provider
            .fetch("usps", "9400100000")
            .unwrap_err()
            .contains("--proxy"));
    }
}
