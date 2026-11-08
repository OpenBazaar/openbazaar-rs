//! Routes for the own store's coupon codes, for paired loopback callers.
//!
//! - `GET /coupons`: every code with its terms and how often it was used.
//! - `POST /coupons`: sign a code (replacing one with the same code).
//! - `DELETE /coupons/{code}`: withdraw a code; orders naming it are declined.
//! - `GET /offers`: bids waiting for an answer.
//! - `POST /offers`: a buyer seals one bid to the listing's store.
//! - `POST /offers/{id}/answer`: decline a bid, or sign an offer for it.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::own_listings::StoreError;
use crate::store_http::error_response;
use crate::Node;

const MAX_JSON: usize = 16 * 1024;

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    if let Some(response) = offers(node, client, head, method, path, body, origin) {
        return Some(response);
    }
    let code = path.strip_prefix("/coupons/").filter(|rest| !rest.is_empty() && !rest.contains('/'));
    let known = matches!((method, path == "/coupons", code.is_some()), ("GET", true, _) | ("POST", true, _) | ("DELETE", _, true));
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    Some(match (method, code) {
        ("GET", _) => json_body("200 OK", &node.own_coupons_value(), origin),
        ("POST", _) => {
            let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
            match parsed.map(|value| node.create_coupon(&value)) {
                Some(Ok(value)) => json_body("200 OK", &value, origin),
                Some(Err(err)) => error_response(err, origin),
                None => json_error("400 Bad Request", "The coupon could not be read.", origin),
            }
        }
        (_, Some(code)) => match node.withdraw_coupon(code) {
            Ok(()) => json_body("200 OK", &json!({ "ok": true }), origin),
            Err(StoreError::NotFound) => json_error("404 Not Found", "That coupon is not in your store.", origin),
            Err(err) => error_response(err, origin),
        },
        _ => json_error("404 Not Found", "not found", origin),
    })
}

fn offers(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    let answer = path.strip_prefix("/offers/").and_then(|rest| rest.strip_suffix("/answer")).filter(|id| !id.is_empty() && !id.contains('/'));
    let submit = method == "POST" && path == "/offers";
    let known = (method == "GET" && path == "/offers") || submit || (method == "POST" && answer.is_some());
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    if submit {
        let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
        return Some(match parsed.map(|value| node.submit_bid(&value)) {
            Some(Ok(value)) => json_body("200 OK", &value, origin),
            Some(Err(err)) => error_response(err, origin),
            None => json_error("400 Bad Request", "The offer could not be read.", origin),
        });
    }
    Some(match answer {
        None => json_body("200 OK", &node.own_offers_value(), origin),
        Some(id) => {
            let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
            match parsed.map(|value| node.answer_offer(id, &value)) {
                Some(Ok(value)) => json_body("200 OK", &value, origin),
                Some(Err(StoreError::NotFound)) => json_error("404 Not Found", "That offer is not waiting for an answer.", origin),
                Some(Err(err)) => error_response(err, origin),
                None => json_error("400 Bad Request", "The answer could not be read.", origin),
            }
        }
    })
}
