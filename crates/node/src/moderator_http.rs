//! Routes for moderator identities.
//!
//! Paired loopback callers and paired devices:
//!
//! - `GET /moderator/profile`, `PUT /moderator/profile`: this node's own
//!   moderator identity ([`crate::moderator_identity`]). Reading works while
//!   the wallet is locked. Changing it signs, so it needs the wallet unlocked.
//! - `GET /moderators`: the moderators this node's stores accept and the
//!   others it knows. `POST /moderators` accepts one, `DELETE
//!   /moderators/{id}` stops accepting one ([`crate::moderators`]).
//! - `GET /stores/{key}/moderators`: the moderators a store accepts, for the
//!   buyer to choose from at checkout.
//!
//! Other nodes, with no token, and never a browser page:
//!
//! - `GET /public/moderator`: this node's signed moderator profile and the
//!   public rulings its identity signed.
//! - `GET /public/moderators`: the ids of the moderators this node's stores
//!   accept, and the signed profiles it holds for them.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::own_listings::StoreError;
use crate::Node;

const MAX_JSON: usize = 16 * 1024;

fn result(result: Result<Value, StoreError>, missing: &str, origin: Option<&str>) -> Vec<u8> {
    match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(StoreError::NotFound) => json_error("404 Not Found", missing, origin),
        Err(err) => crate::store_http::error_response(err, origin),
    }
}

fn read_json(body: &[u8]) -> Result<Value, StoreError> {
    if body.len() > MAX_JSON {
        return Err(StoreError::invalid("That request is too large."));
    }
    serde_json::from_slice(body).map_err(|_| StoreError::invalid("That request could not be read."))
}

/// The route for `method` and `path`, or `None` when this module has none.
pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    const NOT_MODERATING: &str = "This node does not moderate.";
    const NOT_ACCEPTED: &str = "Your store does not accept that moderator.";
    let removed = path.strip_prefix("/moderators/");
    let store = path.strip_prefix("/stores/").and_then(|rest| rest.strip_suffix("/moderators"));
    match (method, path) {
        ("GET", "/public/moderator") => return Some(public(head, node.public_moderator_value(), NOT_MODERATING)),
        ("GET", "/public/moderators") => {
            let accepted = node.builtin_moderator().map(|_| {
                let ids: Vec<String> = node.accepted_moderators().iter().map(|id| crate::escrow_mail::hex(id)).collect();
                json!({ "accepted": ids, "moderators": node.accepted_profiles() })
            });
            return Some(public(head, accepted, crate::instructions::ESCROW_MAINNET_REFUSAL));
        }
        ("GET" | "PUT", "/moderator/profile") | ("GET" | "POST", "/moderators") => {}
        ("DELETE", _) if removed.is_some() => {}
        ("GET", _) if store.is_some() => {}
        _ => return None,
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    Some(match (method, path) {
        ("GET", "/moderator/profile") => result(node.moderator_profile_value(), NOT_MODERATING, origin),
        ("PUT", "/moderator/profile") => {
            result(read_json(body).and_then(|value| node.update_moderator_profile(&value)), NOT_MODERATING, origin)
        }
        ("GET", "/moderators") => json_body("200 OK", &node.moderators_value(), origin),
        ("POST", "/moderators") => result(read_json(body).and_then(|value| node.accept_moderator(&value)), NOT_ACCEPTED, origin),
        ("DELETE", _) => result(node.remove_moderator(removed.unwrap_or_default()), NOT_ACCEPTED, origin),
        _ => result(
            node.store_moderators_value(store.unwrap_or_default()).ok_or(StoreError::NotFound),
            "This node has no record of that store.",
            origin,
        ),
    })
}

fn public(head: &str, value: Option<Value>, missing: &str) -> Vec<u8> {
    crate::public_http::refuse_pages(head).unwrap_or_else(|| match value {
        Some(value) => json_body("200 OK", &value, None),
        None => json_error("404 Not Found", missing, None),
    })
}
