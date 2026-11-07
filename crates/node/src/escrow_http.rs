//! Escrow routes.
//!
//! `POST /escrow/mail` is the peer mail drop: one raw sealed letter, with no
//! pairing token. The shop accepts it from a loopback peer. The sale onion
//! reaches it only through the gate, which is that peer. The letter
//! is only stored; it is opened later by the mail loop. Everything else is
//! for this node's own shop or operator and needs the pairing token.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::pay_order::PayError;
use crate::Node;

pub(crate) fn mail_response(
    node: &Node,
    client: &Client,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if !client.loopback {
        return json_error(
            "403 Forbidden",
            "Escrow mail is loopback-only for now.",
            origin,
        );
    }
    match node.accept_escrow_mail(body) {
        Ok(id) => json_body("202 Accepted", &json!({ "accepted": id }), origin),
        Err(message) => json_error("400 Bad Request", &message, origin),
    }
}

pub(crate) fn sales_response(
    node: &Node,
    client: &Client,
    head: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    json_body("200 OK", &node.sales_value(), origin)
}

pub(crate) fn disputes_response(
    node: &Node,
    client: &Client,
    head: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    json_body("200 OK", &node.disputes_value(), origin)
}

/// The escrow actions that take a JSON body and answer with a [`PayError`].
pub(crate) enum Action<'a> {
    Release(&'a str),
    Dispute(&'a str),
    Rule(&'a str),
    SaleDispute(&'a str),
    Bump,
}

pub(crate) fn action_response(
    node: &Node,
    client: &Client,
    head: &str,
    action: Action<'_>,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    if body.len() > 4 * 1024 {
        return json_error("400 Bad Request", "That request is too large.", origin);
    }
    let parsed: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => {
                return json_error("400 Bad Request", "The request could not be read.", origin)
            }
        }
    };
    let result = match action {
        Action::Release(id) => node.release_escrow(id),
        Action::Dispute(id) => node.dispute_escrow(id, &parsed),
        Action::Rule(id) => node.rule_dispute(id, &parsed),
        Action::SaleDispute(id) => node.dispute_sale(id, &parsed),
        Action::Bump => node.bump_escrow(&parsed),
    };
    match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(PayError::NotFound) => {
            json_error("404 Not Found", "That escrow is not on this node.", origin)
        }
        Err(PayError::Invalid(message)) => json_error("400 Bad Request", &message, origin),
        Err(PayError::Conflict(message)) => json_error("409 Conflict", &message, origin),
        Err(PayError::Chain(message)) => json_error("502 Bad Gateway", &message, origin),
        Err(PayError::Locked(state)) => crate::lock_http::locked_response(state, origin),
        Err(PayError::Failed(detail)) => {
            eprintln!("ob-node: escrow: {detail}");
            json_error("500 Internal Server Error", "The escrow step failed. Details are in the node log.", origin)
        }
    }
}
