//! Network, wallet, and pay routes. Everything except `GET /network` needs a
//! loopback caller with the pairing token.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::pay_order::PayError;
use crate::Node;

pub(crate) fn network_response(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let chain = node.chain.status();
    let mut value = json!({
        "network": node.network.as_str(),
        "confirmations": node.settings.confirmations,
        "publicChainServer": chain.public_server(),
        "chainBackend": chain.to_json(),
        "xmrBackend": node.xmr.backend_value(),
        "lightning": node.ln_wallet_value(),
        "lightningBackend": node.ln_backend_value(),
        "silentPayments": "code-only",
        "letterOnion": node.advertised_onion(),
        "onion": node.advertised_onion(),
        "moderates": node.escrow.config.moderates,
    });
    if let (Some(map), Value::Object(mut egress)) =
        (value.as_object_mut(), node.local.egress.value(chain.public_server()))
    {
        if let Some(proxy) = node.local.egress.proxy.as_deref() {
            egress.insert("proxyReachable".into(), json!(crate::egress::Egress::proxy_reachable(proxy)));
        }
        map.extend(egress);
    }
    json_body("200 OK", &value, origin)
}

pub(crate) fn wallet_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let labels = payment_labels(node);
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
            "activity": wallet.activity_value(&labels),
        }),
        origin,
    )
}

/// Seller names for payments this node sent, and the directory fee when it has one.
fn payment_labels(node: &Node) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    {
        let orders = node.orders();
        for order in orders.on_network() {
            if let Some(txid) = &order.pay_txid {
                labels.insert(txid.clone(), format!("Payment to {}", order.seller));
            }
        }
    }
    if let Some(txid) = node.directory_fee_txid() {
        labels.entry(txid).or_insert_with(|| "Directory fee".to_string());
    }
    labels
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
    order_action(node, client, head, id, body, origin, "202 Accepted", Node::queue_payment)
}

/// `POST /orders/{id}/pay/bump` ([`crate::pay_bump`]).
pub(crate) fn bump_response(
    node: &Node,
    client: &Client,
    head: &str,
    id: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    order_action(node, client, head, id, body, origin, "200 OK", Node::bump_payment)
}

#[allow(clippy::too_many_arguments)]
fn order_action(
    node: &Node,
    client: &Client,
    head: &str,
    id: &str,
    body: &[u8],
    origin: Option<&str>,
    ok: &str,
    action: fn(&Node, &str, &Value) -> Result<Value, PayError>,
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
    match action(node, id, &parsed) {
        Ok(value) => json_body(ok, &value, origin),
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
