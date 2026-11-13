//! Monero routes. All need a loopback caller with the pairing token.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `GET /xmr/wallet` | any | | balance, primary address, sync state |
//! | `POST /xmr/wallet/subaddress` | any | | a fresh subaddress of the node wallet |
//! | `GET /xmr/stores` | any | | which stores accept Monero |
//! | `PUT /xmr/stores` | any (the node's own store: unlocked) | `{seller, accept}` | the same, after the store's record is signed again |
//! | `POST /xmr/orders/{id}/pay/quote` | unlocked | | `{quote, ...terms}` |
//! | `POST /xmr/spend-tickets` | unlocked | `{quote, passphrase}` | `{ticket, ...terms}` |
//! | `POST /xmr/orders/{id}/pay` | unlocked | `{ticket, confirm?}` | 202, the order with `paying: true` |
//!
//! Amounts are piconero as decimal strings (`"amountPiconero"`), with the
//! same amount in XMR alongside (`"amountXmr"`). Errors follow the bitcoin
//! routes: 423 locked, 400 wrong passphrase, 429 backoff, 409 stale ticket.

use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::lock_http::{lock_error, pay_error};
use crate::spend_tickets::TicketError;
use crate::xmr::AcceptError;
use crate::Node;

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

pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, path: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let rest = &path["/xmr".len()..];
    let order = |suffix: &str| rest.strip_prefix("/orders/").and_then(|tail| tail.strip_suffix(suffix)).filter(|id| !id.contains('/'));
    match (method, rest) {
        ("GET", "/wallet") => wallet(node, origin),
        ("POST", "/wallet/subaddress") => subaddress(node, origin),
        ("GET", "/stores") => json_body("200 OK", &node.xmr_stores_value(), origin),
        ("PUT", "/stores") => match read_body(body, &["seller", "accept"], origin) {
            Ok(map) => {
                let (Some(seller), Some(accept)) = (map.get("seller").and_then(Value::as_str), map.get("accept").and_then(Value::as_bool)) else {
                    return json_error("400 Bad Request", "Send {\"seller\": NAME, \"accept\": true or false}.", origin);
                };
                match node.set_accepts_xmr(seller, accept) {
                    Ok(()) => json_body("200 OK", &node.xmr_stores_value(), origin),
                    Err(AcceptError::Unknown(message)) => json_error("404 Not Found", &message, origin),
                    Err(AcceptError::Off) => json_error("409 Conflict", crate::xmr::NEEDS_DAEMON, origin),
                    Err(AcceptError::Locked) => crate::lock_http::locked_response(node.lock.state().as_str(), origin),
                    Err(AcceptError::Failed(detail)) => {
                        eprintln!("ob-node: {detail}");
                        json_error("500 Internal Server Error", "The store could not be signed again.", origin)
                    }
                }
            }
            Err(response) => response,
        },
        ("POST", "/spend-tickets") => match read_body(body, &["quote", "passphrase"], origin) {
            Ok(map) => ticket(node, &map, origin),
            Err(response) => response,
        },
        ("POST", _) if order("/pay/quote").is_some() => match node.xmr_quote(order("/pay/quote").unwrap_or_default()) {
            Ok((quote, terms)) => {
                let mut value = terms.value(node.xmr.xmr_network().as_str());
                value["quote"] = json!(quote);
                json_body("200 OK", &value, origin)
            }
            Err(err) => pay_error(err, origin),
        },
        ("POST", _) if order("/pay").is_some() => match read_body(body, &["ticket", "confirm"], origin) {
            Ok(map) => match node.xmr_queue_payment(order("/pay").unwrap_or_default(), &Value::Object(map)) {
                Ok(value) => json_body("202 Accepted", &value, origin),
                Err(err) => pay_error(err, origin),
            },
            Err(response) => response,
        },
        _ => json_error("404 Not Found", "not found", origin),
    }
}

fn wallet(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let network = node.xmr.xmr_network().as_str();
    let slot = node.xmr.buyer();
    let Some(wallet) = &slot.wallet else {
        return json_body(
            "200 OK",
            &json!({ "network": network, "enabled": node.xmr.chain().is_some(), "keys": "unlock-to-create", "balance": null }),
            origin,
        );
    };
    let balance = wallet.balance();
    json_body(
        "200 OK",
        &json!({
            "network": network,
            "enabled": node.xmr.chain().is_some(),
            "keys": "ready",
            "primaryAddress": wallet.primary_address().to_string(),
            "balance": {
                "unlockedPiconero": balance.unlocked.to_string(),
                "pendingPiconero": balance.pending.to_string(),
                "unlockedXmr": openbazaar_xmr::amount::format_xmr(balance.unlocked),
                "pendingXmr": openbazaar_xmr::amount::format_xmr(balance.pending),
            },
            "scannedHeight": wallet.scanned(),
            "tip": wallet.tip(),
            "lastSync": slot.last_sync,
            "syncError": slot.error,
        }),
        origin,
    )
}

fn subaddress(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let mut slot = node.xmr.buyer();
    let Some(wallet) = slot.wallet.as_mut() else {
        return crate::lock_http::locked_response(crate::wallet_lock::State::Locked.as_str(), origin);
    };
    let (index, address) = wallet.issue_subaddress();
    if let Err(err) = slot.save(&node.xmr.vault) {
        eprintln!("ob-node: {err}");
        return json_error("500 Internal Server Error", "A new subaddress could not be saved.", origin);
    }
    json_body("200 OK", &json!({ "network": node.xmr.xmr_network().as_str(), "index": index, "address": address.to_string() }), origin)
}

fn ticket(node: &Node, map: &Map<String, Value>, origin: Option<&str>) -> Vec<u8> {
    let text = |key: &str| map.get(key).and_then(Value::as_str).unwrap_or("");
    if let Err(err) = node.lock.verify(text("passphrase")) {
        return lock_error(err, origin);
    }
    match node.lock.xmr_tickets.issue(text("quote"), node.lock.now()) {
        Ok((ticket, terms)) => {
            let mut value = terms.value(node.xmr.xmr_network().as_str());
            value["ticket"] = json!(ticket);
            json_body("200 OK", &value, origin)
        }
        Err(TicketError::Unknown | TicketError::OtherOrder) => {
            json_error("409 Conflict", "That quote is used or expired. Ask for a new one.", origin)
        }
    }
}
