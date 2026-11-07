//! Routes for orders between nodes.
//!
//! - `POST /mail`: order letters and buyer messages from other nodes.
//! - `POST /mail/reply`: seller replies to this node's orders.
//! - `GET /sales`, `GET /sales/{id}`, `POST /sales/{id}/fulfill`: the seller's sales.
//! - `POST /orders/{id}/complete`: the buyer confirms an order arrived.
//!
//! The two mail routes take no token, like `POST /escrow/mail`, under the
//! same limits ([`crate::mail_drop`]). The rest are for paired loopback callers.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::order_mail::{MailError, REPLY_MAIL, STORE_MAIL};
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
    if method == "POST" && (path == STORE_MAIL || path == REPLY_MAIL) {
        if let Err(response) = mail_drop::admit_mail(node, head, body) {
            return Some(response);
        }
        let result = if path == STORE_MAIL { node.receive_store_mail(body) } else { node.receive_reply(body) };
        return Some(match result {
            Ok(()) => json_body("200 OK", &json!({ "ok": true }), None),
            Err(MailError::Refused(message)) => json_error("400 Bad Request", &message, None),
            Err(MailError::Retry(detail)) => {
                eprintln!("ob-node: mail: {detail}");
                json_error("503 Service Unavailable", "Try again later.", None)
            }
        });
    }
    let fulfill = path.strip_prefix("/sales/").and_then(|rest| rest.strip_suffix("/fulfill"));
    let complete = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/complete"));
    let sale = path.strip_prefix("/sales/").filter(|rest| !rest.contains('/'));
    let known = match method {
        "GET" => path == "/sales" || sale.is_some(),
        "POST" => fulfill.is_some() || complete.is_some(),
        _ => false,
    };
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
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
