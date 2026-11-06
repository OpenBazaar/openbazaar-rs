//! Network, wallet, and pay routes. Everything except `GET /network` needs a
//! loopback caller with the pairing token.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::pay_order::PayError;
use crate::Node;

pub(crate) fn network_response(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let mut value = json!({
        "network": node.network.as_str(),
        "confirmations": node.settings.confirmations,
        "publicChainServer": node.settings.public_server,
        "lightning": "offer-only",
        "silentPayments": "code-only",
    });
    if let (Some(map), Value::Object(egress)) = (value.as_object_mut(), node.local.egress.value(node.settings.public_server)) {
        map.extend(egress);
    }
    json_body("200 OK", &value, origin)
}

pub(crate) fn wallet_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let mut wallet = node.buyer_wallet();
    let address = match wallet.receive_address() {
        Ok(address) => address,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return json_error("500 Internal Server Error", "The wallet could not be read.", origin);
        }
    };
    json_body(
        "200 OK",
        &json!({
            "network": node.network.as_str(),
            "balance": wallet.balance_value(),
            "receiveAddress": address.to_string(),
            "lastSync": wallet.last_sync,
            "syncError": wallet.sync_error,
        }),
        origin,
    )
}

pub(crate) fn wallet_address_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    match node.buyer_wallet().fresh_address() {
        Ok(address) => json_body(
            "200 OK",
            &json!({ "network": node.network.as_str(), "address": address.to_string() }),
            origin,
        ),
        Err(err) => {
            eprintln!("ob-node: {err}");
            json_error("500 Internal Server Error", "A new address could not be saved.", origin)
        }
    }
}

pub(crate) fn pay_response(
    node: &Node,
    client: &Client,
    head: &str,
    id: &str,
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
            Err(_) => return json_error("400 Bad Request", "The request could not be read.", origin),
        }
    };
    match node.queue_payment(id, &parsed) {
        Ok(value) => json_body("202 Accepted", &value, origin),
        Err(PayError::NotFound) => json_error("404 Not Found", "That order is not on this node.", origin),
        Err(PayError::Invalid(message)) => json_error("400 Bad Request", &message, origin),
        Err(PayError::Conflict(message)) => json_error("409 Conflict", &message, origin),
        Err(PayError::Chain(message)) => json_error("502 Bad Gateway", &message, origin),
        Err(PayError::Locked(state)) => crate::lock_http::locked_response(state, origin),
        Err(PayError::Failed(detail)) => {
            eprintln!("ob-node: pay {id}: {detail}");
            json_error("500 Internal Server Error", "The payment failed. Details are in the node log.", origin)
        }
    }
}
