//! Zcash routes. All need a loopback caller with the pairing token.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `GET /zec/wallet` | any | | balance, receive address, sync state |
//! | `POST /zec/wallet/send/quote` | unlocked | `{address, amountZatoshi}` | `{quote, ...terms}` on testnet. Mainnet is refused. |
//! | `POST /zec/wallet/send` | unlocked | `{quote, passphrase, confirm: true}` | the terms with `txid` |
//! | `GET /zec/stores` | any | | which stores accept Zcash |
//! | `PUT /zec/stores` | any; unlocked for the own store | `{seller, accept}` | the same, after the store's record is signed again |
//! | `POST /zec/orders/{id}/pay/quote` | unlocked | | `{quote, ...terms}` |
//! | `POST /zec/spend-tickets` | unlocked | `{quote, passphrase}` | `{ticket, ...terms}` |
//! | `POST /zec/orders/{id}/pay` | unlocked | `{ticket, confirm?}` | 202, the order with `paying: true` |
//!
//! Turning the own store on answers 403 on mainnet, 409 while it could not
//! take a Zcash order (no `--zec-indexer`, the chain not read yet, no ZEC
//! price), and 423 while locked (see `own.rs`). Order amounts
//! are zatoshi as decimal strings (`"amountZatoshi"`), with the same amount
//! in ZEC alongside (`"amountZec"`). A payment from this node spends confirmed
//! Ironwood notes only.

use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::lock_http::{lock_error, pay_error};
use crate::spend_tickets::TicketError;
use crate::zec::own::NotReady;
use crate::zec::AcceptError;
use crate::Node;

const MAX_BODY: usize = 4 * 1024;

fn read_body(
    body: &[u8],
    allowed: &[&str],
    origin: Option<&str>,
) -> Result<Map<String, Value>, Vec<u8>> {
    if body.len() > MAX_BODY {
        return Err(json_error(
            "400 Bad Request",
            "That request is too large.",
            origin,
        ));
    }
    let value: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        serde_json::from_slice(body)
            .map_err(|_| json_error("400 Bad Request", "The request could not be read.", origin))?
    };
    match value {
        Value::Object(map) if map.keys().all(|key| allowed.contains(&key.as_str())) => Ok(map),
        _ => Err(json_error(
            "400 Bad Request",
            "That field is not part of this request.",
            origin,
        )),
    }
}

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let rest = &path["/zec".len()..];
    let order = |suffix: &str| {
        rest.strip_prefix("/orders/")
            .and_then(|tail| tail.strip_suffix(suffix))
            .filter(|id| !id.contains('/'))
    };
    match (method, rest) {
        ("GET", "/wallet") => wallet(node, origin),
        ("POST", "/enable") => match read_body(body, &["indexers"], origin) {
            Ok(map) => {
                let raw = map.get("indexers").and_then(Value::as_str).unwrap_or("").trim();
                let urls: Vec<String> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                if urls.is_empty() {
                    return json_error("400 Bad Request", "Provide at least one Zcash indexer URL (for example http://127.0.0.1:8137).", origin);
                }
                let config = match openbazaar_zec::IndexerConfig::new(node.network.zcash(), &urls, node.local.egress.proxy.clone()) {
                    Ok(config) => config,
                    Err(err) => return json_error("400 Bad Request", &err.to_string(), origin),
                };
                let chain = match openbazaar_zec::IndexerChain::new(config) {
                    Ok(chain) => chain,
                    Err(err) => return json_error("400 Bad Request", &err.to_string(), origin),
                };
                match node.set_zec_chain(std::sync::Arc::new(chain)) {
                    Ok(()) => json_body("200 OK", &node.zec.backend_value(), origin),
                    Err(err) => json_error("409 Conflict", &err, origin),
                }
            }
            Err(response) => response,
        },
        ("POST", "/wallet/send/quote") => match read_body(body, &["address", "amountZatoshi"], origin) {
            Ok(map) => match node.zec_send_quote(&map) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(err) => pay_error(err, origin),
            },
            Err(response) => response,
        },
        ("POST", "/wallet/send") => crate::wallet_send::confirmed_response(node, body, origin, |quote| node.zec_send_quoted(quote)),
        ("GET", "/stores") => json_body("200 OK", &node.zec_stores_value(), origin),
        ("PUT", "/stores") => match read_body(body, &["seller", "accept"], origin) {
            Ok(map) => {
                let (Some(seller), Some(accept)) = (
                    map.get("seller").and_then(Value::as_str),
                    map.get("accept").and_then(Value::as_bool),
                ) else {
                    return json_error(
                        "400 Bad Request",
                        "Send {\"seller\": NAME, \"accept\": true or false}.",
                        origin,
                    );
                };
                match node.set_accepts_zec(seller, accept) {
                    Ok(()) => json_body("200 OK", &node.zec_stores_value(), origin),
                    Err(AcceptError::Unknown(message)) => {
                        json_error("404 Not Found", &message, origin)
                    }
                    Err(AcceptError::NotReady(NotReady::Network)) => {
                        json_error("403 Forbidden", NotReady::Network.vendor(), origin)
                    }
                    Err(AcceptError::NotReady(why)) => {
                        json_error("409 Conflict", why.vendor(), origin)
                    }
                    Err(AcceptError::Locked) => {
                        crate::lock_http::locked_response(node.lock.state().as_str(), origin)
                    }
                    Err(AcceptError::Failed(detail)) => {
                        eprintln!("ob-node: {detail}");
                        json_error(
                            "500 Internal Server Error",
                            "The store could not be signed again.",
                            origin,
                        )
                    }
                }
            }
            Err(response) => response,
        },
        ("POST", "/spend-tickets") => match read_body(body, &["quote", "passphrase"], origin) {
            Ok(map) => ticket(node, &map, origin),
            Err(response) => response,
        },
        ("POST", _) if order("/pay/quote").is_some() => {
            match node.zec_pay_quote(order("/pay/quote").unwrap_or_default()) {
                Ok((quote, terms)) => {
                    let mut value = terms.value(node.zec.zec_network().as_str());
                    value["quote"] = json!(quote);
                    json_body("200 OK", &value, origin)
                }
                Err(err) => pay_error(err, origin),
            }
        }
        ("POST", _) if order("/pay").is_some() => {
            match read_body(body, &["ticket", "confirm"], origin) {
                Ok(map) => match node
                    .zec_queue_payment(order("/pay").unwrap_or_default(), &Value::Object(map))
                {
                    Ok(value) => json_body("202 Accepted", &value, origin),
                    Err(err) => pay_error(err, origin),
                },
                Err(response) => response,
            }
        }
        _ => json_error("404 Not Found", "There is no such Zcash route.", origin),
    }
}

fn wallet(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let network = node.zec.zec_network().as_str();
    let slot = node.zec.own();
    let Some(wallet) = &slot.wallet else {
        return json_body(
            "200 OK",
            &json!({ "network": network, "enabled": node.zec.chain().is_some(), "keys": "unlock-to-create", "balance": null }),
            origin,
        );
    };
    let balance = wallet.balance();
    json_body(
        "200 OK",
        &json!({
            "network": network,
            "enabled": node.zec.chain().is_some(),
            "keys": "ready",
            "receiveAddress": wallet.receive_address(),
            "balance": {
                "confirmedZatoshi": balance.confirmed.to_string(),
                "pendingZatoshi": balance.pending.to_string(),
                "orchardZatoshi": balance.orchard.to_string(),
                "confirmedZec": openbazaar_zec::amount::format_zec(balance.confirmed),
                "pendingZec": openbazaar_zec::amount::format_zec(balance.pending),
                "orchardZec": openbazaar_zec::amount::format_zec(balance.orchard),
            },
            "scannedHeight": wallet.scanned(),
            "tip": wallet.tip(),
            "lastSync": slot.last_sync,
            "syncError": slot.error,
        }),
        origin,
    )
}

fn ticket(node: &Node, map: &Map<String, Value>, origin: Option<&str>) -> Vec<u8> {
    let text = |key: &str| map.get(key).and_then(Value::as_str).unwrap_or("");
    if let Err(err) = node.lock.verify(text("passphrase")) {
        return lock_error(err, origin);
    }
    match node.lock.zec_tickets.issue(text("quote"), node.lock.now()) {
        Ok((ticket, terms)) => {
            let mut value = terms.value(node.zec.zec_network().as_str());
            value["ticket"] = json!(ticket);
            json_body("200 OK", &value, origin)
        }
        Err(TicketError::Unknown | TicketError::OtherOrder) => json_error(
            "409 Conflict",
            "That quote is used or expired. Ask for a new one.",
            origin,
        ),
    }
}
