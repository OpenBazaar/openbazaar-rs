//! Routes for orders between nodes.
//!
//! - `POST /mail`: order letters and buyer messages from other nodes.
//! - `POST /mail/reply`: seller replies to this node's orders.
//! - `GET /sales`, `GET /sales/{id}`, `POST /sales/{id}/fulfill`: the seller's sales.
//! - `POST /orders/{id}/complete`: the buyer confirms an order arrived.
//! - `POST /sales/{id}/message`, `POST /orders/{id}/message`: the two sides
//!   of an order write to each other (see [`crate::messages`]).
//! - `POST /orders/{id}/return`, `POST /sales/{id}/return`, and
//!   `POST /orders/{id}/return/shipment`: a return case on the same channel
//!   (see [`crate::return_case`]).
//!
//! The two mail routes take no token, like `POST /escrow/mail`, under the
//! same limits ([`crate::mail_drop`]). The rest are for paired loopback callers.

use serde_json::{json, Value};

use crate::escrow_mail::hex;
use crate::http::{json_body, json_error, local_guard, Client};
use crate::order_mail::{MailError, PULL_MAIL, REPLY_MAIL, STORE_MAIL};
use crate::{lock_http, mail_drop, Node, OrderError};

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    if method == "POST" && (path == STORE_MAIL || path == REPLY_MAIL || path == PULL_MAIL) {
        if let Err(response) = mail_drop::admit_mail(node, head, body) {
            return Some(response);
        }
        if path == PULL_MAIL {
            return Some(match node.pull_sale_status(body) {
                Ok(value) => json_body("200 OK", &value, None),
                Err(MailError::Unauthorized(message)) => json_error("403 Forbidden", &message, None),
                Err(MailError::Refused(message)) => json_error("400 Bad Request", &message, None),
                Err(MailError::Retry(detail)) => {
                    eprintln!("ob-node: mail pull: {detail}");
                    json_error("503 Service Unavailable", "Try again later.", None)
                }
            });
        }
        if path == STORE_MAIL {
            return Some(match node.receive_store_mail(body) {
                Ok(Some(reply)) => json_body("200 OK", &json!({ "ok": true, "reply": hex(&reply.to_bytes()) }), None),
                Ok(None) => json_body("200 OK", &json!({ "ok": true }), None),
                Err(MailError::Unauthorized(message)) => json_error("403 Forbidden", &message, None),
                Err(MailError::Refused(message)) => json_error("400 Bad Request", &message, None),
                Err(MailError::Retry(detail)) => {
                    eprintln!("ob-node: mail: {detail}");
                    json_error("503 Service Unavailable", "Try again later.", None)
                }
            });
        }
        return Some(match node.receive_reply(body) {
            Ok(()) => json_body("200 OK", &json!({ "ok": true }), None),
            Err(MailError::Unauthorized(message)) => json_error("403 Forbidden", &message, None),
            Err(MailError::Refused(message)) => json_error("400 Bad Request", &message, None),
            Err(MailError::Retry(detail)) => {
                eprintln!("ob-node: mail: {detail}");
                json_error("503 Service Unavailable", "Try again later.", None)
            }
        });
    }
    let fulfill = path.strip_prefix("/sales/").and_then(|rest| rest.strip_suffix("/fulfill"));
    let complete = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/complete"));
    let order_pull = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/pull")).filter(|id| !id.is_empty() && !id.contains('/'));
    let order_contract = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/contract")).filter(|id| !id.is_empty() && !id.contains('/'));
    let sale_message = path.strip_prefix("/sales/").and_then(|rest| rest.strip_suffix("/message"));
    let order_message = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/message"));
    let order_return_ship = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/return/shipment")).filter(|id| !id.is_empty() && !id.contains('/'));
    let order_return = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/return")).filter(|id| !id.is_empty() && !id.contains('/'));
    let sale_return = path.strip_prefix("/sales/").and_then(|rest| rest.strip_suffix("/return")).filter(|id| !id.is_empty() && !id.contains('/'));
    let sale = path.strip_prefix("/sales/").filter(|rest| !rest.contains('/'));
    let known = match method {
        "GET" => path == "/sales" || sale.is_some() || order_contract.is_some(),
        "POST" => {
            fulfill.is_some()
                || complete.is_some()
                || order_pull.is_some()
                || sale_message.is_some()
                || order_message.is_some()
                || order_return_ship.is_some()
                || order_return.is_some()
                || sale_return.is_some()
        }
        _ => false,
    };
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    if method == "GET" {
        if let Some(id) = order_contract {
            return Some(match node.export_retained_contract(id) {
                Ok(contract) => json_body("200 OK", &contract.to_json(), origin),
                Err(message) if message.contains("not on this node") => json_error("404 Not Found", &message, origin),
                Err(message) => json_error("400 Bad Request", &message, origin),
            });
        }
    }
    if method == "POST" {
        if let Some(id) = order_pull {
            return Some(match node.pull_remote_order(id) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(OrderError::Invalid(message)) => json_error("400 Bad Request", message, origin),
                Err(OrderError::Locked(state)) => lock_http::locked_response(state, origin),
                Err(OrderError::Failed(message)) => json_error("502 Bad Gateway", &message, origin),
                Err(OrderError::UnknownListing) => json_error("404 Not Found", "That order is not on this node.", origin),
            });
        }
    }
    let message = |send: &dyn Fn(&str, &Value) -> Result<Value, String>| -> Option<Vec<u8>> {
        let id = sale_message.or(order_message)?;
        let parsed = if body.len() > 16 * 1024 { None } else { serde_json::from_slice::<Value>(body).ok() };
        Some(match parsed {
            Some(value) => match send(id, &value) {
                Ok(value) => json_body("200 OK", &value, origin),
                Err(message) => json_error("400 Bad Request", &message, origin),
            },
            None => json_error("400 Bad Request", "The message could not be read.", origin),
        })
    };
    if method == "POST" && (order_return_ship.is_some() || order_return.is_some() || sale_return.is_some()) {
        let id = order_return_ship.or(order_return).or(sale_return).unwrap_or_default();
        let parsed = if body.len() > 16 * 1024 { None } else { serde_json::from_slice::<Value>(body).ok() };
        let result = match parsed {
            Some(value) if order_return_ship.is_some() => node.ship_return(id, &value).map_err(Some),
            Some(value) if order_return.is_some() => node.request_return(id, &value).map_err(Some),
            Some(value) => node.decide_return(id, &value).map_err(|err| match err {
                crate::own_listings::StoreError::NotFound => None,
                crate::own_listings::StoreError::Invalid(message) => Some(message),
                crate::own_listings::StoreError::Locked(_) => Some(crate::lock_http::LOCKED_MESSAGE.to_string()),
                crate::own_listings::StoreError::Failed(detail) => {
                    eprintln!("ob-node: return {id}: {detail}");
                    Some("The return could not be saved.".to_string())
                }
            }),
            None => Err(Some("The return could not be read.".to_string())),
        };
        return Some(match result {
            Ok(value) => json_body("200 OK", &value, origin),
            Err(None) => json_error("404 Not Found", "That sale is not on this node.", origin),
            Err(Some(message)) => json_error("400 Bad Request", &message, origin),
        });
    }
    if method == "POST" && (sale_message.is_some() || order_message.is_some()) {
        return if sale_message.is_some() {
            message(&|id, value| node.send_sale_message(id, value))
        } else {
            message(&|id, value| node.send_order_message(id, value).map_err(|err| match err {
                OrderError::Invalid(message) => message.to_string(),
                err => err.to_string(),
            }))
        };
    }
    Some(match (method, fulfill, complete, sale) {
        ("GET", _, _, None) => json_body("200 OK", &node.own_sales_value(), origin),
        ("GET", _, _, Some(id)) => match node.sale_value(id) {
            Some(value) => json_body("200 OK", &value, origin),
            None => json_error("404 Not Found", "That sale is not on this node.", origin),
        },
        (_, Some(id), _, _) => {
            let parsed = if body.len() > 16 * 1024 { None } else { serde_json::from_slice::<Value>(body).ok() };
            match parsed {
                Some(value) => match node.fulfill_sale(id, &value) {
                    Ok(value) => json_body("200 OK", &value, origin),
                    Err(crate::own_listings::StoreError::NotFound) => {
                        json_error("404 Not Found", "That sale is not on this node.", origin)
                    }
                    Err(err) => crate::store_http::error_response(err, origin),
                },
                None => json_error("400 Bad Request", "The fulfillment could not be read.", origin),
            }
        }
        (_, _, Some(id), _) => match node.complete_order(id) {
            Ok(value) => json_body("200 OK", &value, origin),
            Err(OrderError::Invalid(message)) => json_error("400 Bad Request", message, origin),
            Err(OrderError::Locked(state)) => lock_http::locked_response(state, origin),
            Err(err) => {
                eprintln!("ob-node: {err}");
                json_error("500 Internal Server Error", "The order could not be completed.", origin)
            }
        },
        _ => json_error("404 Not Found", "not found", origin),
    })
}
