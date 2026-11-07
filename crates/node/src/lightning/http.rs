//! Lightning routes. All need a loopback caller with the pairing token.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `GET /ln/status` | any | | node, LSP, balances, channels, who sees what |
//! | `GET /ln/stores` | any | | which stores take Lightning, and the LSP notice |
//! | `PUT /ln/stores` | any (own store: unlocked) | `{seller, accept, acknowledgeLsp?}` | the same, after the record is signed again; 409 with `lspNotice` until acknowledged |
//! | `POST /ln/address` | any | | a fresh address of LDK's on-chain wallet |
//! | `POST /ln/channels` | unlocked | `{peer, sats, passphrase}` | `{channelId}` |
//! | `POST /ln/channels/{id}/close` | unlocked | `{passphrase, force?}` | `{closing: true}` |
//! | `POST /ln/sweep` | unlocked | `{passphrase}` | `{txid, address}` (to the node wallet) |
//! | `POST /ln/orders/{id}/pay/quote` | unlocked | | `{quote, ...terms}` |
//! | `POST /ln/spend-tickets` | unlocked | `{quote, passphrase}` | `{ticket, ...terms}` |
//! | `POST /ln/orders/{id}/pay` | unlocked | `{ticket, confirm?}` | 202, the order with `paying` |
//!
//! Errors follow the bitcoin routes: 423 locked, 400 wrong passphrase or
//! bad request, 429 backoff, 409 stale ticket.

use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::lock_http::{lock_error, locked_response, pay_error};
use crate::Node;

use super::AcceptError;

const MAX_BODY: usize = 4 * 1024;

fn read_body(body: &[u8], allowed: &[&str], origin: Option<&str>) -> Result<Map<String, Value>, Vec<u8>> {
    if body.len() > MAX_BODY {
        return Err(json_error("400 Bad Request", "That request is too large.", origin));
    }
    let value: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(body).map_err(|_| json_error("400 Bad Request", "The request could not be read.", origin))?
    };
    match value {
        Value::Object(map) if map.keys().all(|key| allowed.contains(&key.as_str())) => Ok(map),
        _ => Err(json_error("400 Bad Request", "That field is not part of this request.", origin)),
    }
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, path: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let rest = &path["/ln".len()..];
    let order = |suffix: &str| rest.strip_prefix("/orders/").and_then(|tail| tail.strip_suffix(suffix)).filter(|id| !id.is_empty() && !id.contains('/'));
    let channel = rest.strip_prefix("/channels/").and_then(|tail| tail.strip_suffix("/close")).filter(|id| !id.is_empty() && !id.contains('/'));
    match (method, rest) {
        ("GET", "/status") => json_body("200 OK", &node.ln_status_value(), origin),
        ("GET", "/stores") => json_body("200 OK", &node.ln_stores_value(), origin),
        ("PUT", "/stores") => match read_body(body, &["seller", "accept", "acknowledgeLsp"], origin) {
            Ok(map) => put_store(node, &map, origin),
            Err(response) => response,
        },
        ("POST", "/address") => match node.ln.rail().map(|rail| rail.onchain_address()) {
            Some(Ok(address)) => json_body("200 OK", &json!({ "address": address }), origin),
            Some(Err(err)) => json_error("502 Bad Gateway", &err.to_string(), origin),
            None => json_error("503 Service Unavailable", "The Lightning node is not running.", origin),
        },
        ("POST", "/channels") => match read_body(body, &["peer", "sats", "passphrase"], origin) {
            Ok(map) => {
                let sats = map.get("sats").and_then(Value::as_u64).unwrap_or(0);
                match node.ln_open_channel(text(&map, "peer"), sats, text(&map, "passphrase")) {
                    Ok(value) => json_body("200 OK", &value, origin),
                    Err(err) => pay_error(err, origin),
                }
            }
            Err(response) => response,
        },
        ("POST", _) if channel.is_some() => match read_body(body, &["passphrase", "force"], origin) {
            Ok(map) => {
                let force = map.get("force").and_then(Value::as_bool).unwrap_or(false);
                match node.ln_close_channel(channel.unwrap_or_default(), force, text(&map, "passphrase")) {
                    Ok(value) => json_body("200 OK", &value, origin),
                    Err(err) => pay_error(err, origin),
                }
            }
            Err(response) => response,
        },
        ("POST", "/sweep") => match read_body(body, &["passphrase"], origin) {
            Ok(map) => match node.ln_sweep(text(&map, "passphrase")) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(err) => pay_error(err, origin),
            },
            Err(response) => response,
        },
        ("POST", "/spend-tickets") => match read_body(body, &["quote", "passphrase"], origin) {
            Ok(map) => match node.ln_ticket(text(&map, "quote"), text(&map, "passphrase")) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(crate::wallet_lock::LockError::Invalid(message)) => json_error("409 Conflict", &message, origin),
                Err(err) => lock_error(err, origin),
            },
            Err(response) => response,
        },
        ("POST", _) if order("/pay/quote").is_some() => match node.ln_quote(order("/pay/quote").unwrap_or_default()) {
            Ok((quote, terms)) => {
                let mut value = terms.value(node.ln_network_name());
                value["quote"] = json!(quote);
                json_body("200 OK", &value, origin)
            }
            Err(err) => pay_error(err, origin),
        },
        ("POST", _) if order("/pay").is_some() => match read_body(body, &["ticket", "confirm"], origin) {
            Ok(map) => match node.ln_pay(order("/pay").unwrap_or_default(), &Value::Object(map)) {
                Ok(value) => json_body("202 Accepted", &value, origin),
                Err(err) => pay_error(err, origin),
            },
            Err(response) => response,
        },
        _ => json_error("404 Not Found", "not found", origin),
    }
}

fn put_store(node: &Node, map: &Map<String, Value>, origin: Option<&str>) -> Vec<u8> {
    let (Some(seller), Some(accept)) = (map.get("seller").and_then(Value::as_str), map.get("accept").and_then(Value::as_bool)) else {
        return json_error("400 Bad Request", "Send {\"seller\": NAME, \"accept\": true or false}.", origin);
    };
    let acknowledge = map.get("acknowledgeLsp").and_then(Value::as_bool).unwrap_or(false);
    match node.set_accepts_lightning(seller, accept, acknowledge) {
        Ok(()) => json_body("200 OK", &node.ln_stores_value(), origin),
        Err(AcceptError::Off) => json_error("409 Conflict", "Lightning is off on this node. Start it with --lightning.", origin),
        Err(AcceptError::NeedsAcknowledgement) => json_body(
            "409 Conflict",
            &json!({
                "error": "Read how the LSP sees your payments, then send \"acknowledgeLsp\": true.",
                "lspNotice": super::LSP_NOTICE,
            }),
            origin,
        ),
        Err(AcceptError::Unknown(message)) => json_error("404 Not Found", &message, origin),
        Err(AcceptError::Locked) => locked_response(node.lock.state().as_str(), origin),
        Err(AcceptError::Failed(detail)) => {
            eprintln!("ob-node: {detail}");
            json_error("500 Internal Server Error", "The store could not be signed again.", origin)
        }
    }
}
