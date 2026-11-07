//! Routes for reviews.
//!
//! - `GET /public/reviews`: this node's whole review pool, for other nodes.
//! - `POST /public/reviews`: reviews and replies from another node, kept if they check out.
//! - `GET /stores/{key}/reviews`: one store's reviews and stars, for the app.
//! - `POST /orders/{id}/review`: the buyer writes or changes an order's review.
//! - `POST /reviews/{id}/reply`: the own store answers a review.
//!
//! The public routes take no token and refuse browser pages, like
//! `GET /public/store`; records are checked by signature, not by sender.
//! The rest are for paired loopback callers.

use openbazaar_protocol::StoreId;
use serde_json::Value;

use crate::http::{json_body, json_error, local_guard, Client};
use crate::http_parse::has_header;
use crate::review_publish::PUBLIC_REVIEWS;
use crate::Node;

const MAX_POST: usize = 256 * 1024;
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
    if path == PUBLIC_REVIEWS && (method == "GET" || method == "POST") {
        if has_header(head, "origin") {
            return Some(json_error("403 Forbidden", "Reviews are copied by nodes, not pages.", None));
        }
        if method == "GET" {
            return Some(json_body("200 OK", &node.reviews().records(), None));
        }
        let parsed = if body.len() > MAX_POST { None } else { serde_json::from_slice::<Value>(body).ok() };
        return Some(match parsed {
            Some(value) => json_body("200 OK", &serde_json::json!({ "taken": node.take_reviews(&value) }), None),
            None => json_error("400 Bad Request", "Those reviews could not be read.", None),
        });
    }
    let store = path.strip_prefix("/stores/").and_then(|rest| rest.strip_suffix("/reviews"));
    let write = path.strip_prefix("/orders/").and_then(|rest| rest.strip_suffix("/review"));
    let reply = path.strip_prefix("/reviews/").and_then(|rest| rest.strip_suffix("/reply"));
    let known = matches!((method, store, write, reply), ("GET", Some(_), _, _) | ("POST", _, Some(_), _) | ("POST", _, _, Some(_)));
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    if let Some(key) = store {
        let Some(id) = crate::stores::unhex32(key).map(StoreId::from_bytes) else {
            return Some(json_error("404 Not Found", "That store is not in this shop.", origin));
        };
        let listings = node.catalog().listings.clone();
        return Some(json_body("200 OK", &node.reviews().store_value(&listings, &id), origin));
    }
    let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
    let Some(value) = parsed else {
        return Some(json_error("400 Bad Request", "The review could not be read.", origin));
    };
    let result = match (write, reply) {
        (Some(id), _) => node.write_review(id, &value),
        (_, Some(id)) => node.reply_to_review(id, &value),
        _ => unreachable!("matched above"),
    };
    Some(match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(message) => json_error("400 Bad Request", &message, origin),
    })
}
