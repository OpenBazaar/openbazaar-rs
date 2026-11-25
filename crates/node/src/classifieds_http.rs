//! Routes for the classifieds board and contact threads.
//!
//! - `GET /classifieds`: live posts (optional `?kind=` / `?category=`).
//! - `POST /classifieds`: sign and keep a post from this node's store key.
//! - `GET /classifieds/{id}`: one post.
//! - `DELETE /classifieds/{id}`: drop an own post.
//! - `POST /classifieds/{id}/contact`: open or continue contact mail.
//! - `GET /contact`: contact threads.
//! - `GET /contact/{id}`: one thread.
//! - `POST /contact/{id}/message`: reply in a thread.
//! - `GET|POST /public/classifieds`: node-to-node copy of the board.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::http_parse::has_header;
use crate::own_listings::StoreError;
use crate::store_http::error_response;
use crate::Node;

const MAX_JSON: usize = 16 * 1024;
const MAX_PUBLIC: usize = 256 * 1024;
pub(crate) const PUBLIC_CLASSIFIEDS: &str = "/public/classifieds";

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    if path == PUBLIC_CLASSIFIEDS && (method == "GET" || method == "POST") {
        if has_header(head, "origin") {
            return Some(json_error("403 Forbidden", "Classifieds are copied by nodes, not pages.", None));
        }
        if method == "GET" {
            let today = crate::own_store::today_yyyymmdd();
            let posts: Vec<String> = node
                .classifieds()
                .posts
                .iter()
                .filter(|post| !post.terms.expired_on(today))
                .filter_map(|post| post.encode().ok().map(|bytes| crate::escrow_mail::hex(&bytes)))
                .collect();
            return Some(json_body("200 OK", &json!({ "v": 1, "classifieds": posts }), None));
        }
        let parsed = if body.len() > MAX_PUBLIC { None } else { serde_json::from_slice::<Value>(body).ok() };
        return Some(match parsed {
            Some(value) => {
                let mut taken = 0usize;
                for item in value["classifieds"].as_array().into_iter().flatten().take(500) {
                    if let Some(text) = item.as_str() {
                        if let Ok(bytes) = crate::escrow_mail::unhex(text) {
                            if node.accept_classified(&bytes).unwrap_or(false) {
                                taken += 1;
                            }
                        }
                    }
                }
                json_body("200 OK", &json!({ "taken": taken }), None)
            }
            None => json_error("400 Bad Request", "Those classifieds could not be read.", None),
        });
    }

    if let Some(response) = contact_routes(node, client, head, method, path, body, origin) {
        return Some(response);
    }

    let contact = path.strip_prefix("/classifieds/").and_then(|rest| rest.strip_suffix("/contact")).filter(|id| !id.is_empty() && !id.contains('/'));
    let one = path.strip_prefix("/classifieds/").filter(|rest| !rest.is_empty() && !rest.contains('/') && contact.is_none());
    let list = path == "/classifieds";
    let known = matches!(
        (method, list, one.is_some(), contact.is_some()),
        ("GET", true, _, _) | ("POST", true, _, _) | ("GET", _, true, _) | ("DELETE", _, true, _) | ("POST", _, _, true)
    );
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    if method == "GET" && list {
        let query = request_query(head);
        let kind = query_param(query, "kind");
        let category = query_param(query, "category");
        return Some(json_body("200 OK", &node.classifieds_value(kind, category), origin));
    }
    if method == "GET" {
        return Some(match node.classified_value(one.unwrap()) {
            Some(value) => json_body("200 OK", &value, origin),
            None => json_error("404 Not Found", "That classified is not on this node.", origin),
        });
    }
    if method == "DELETE" {
        return Some(match node.delete_classified(one.unwrap()) {
            Ok(()) => json_body("200 OK", &json!({ "ok": true }), origin),
            Err(StoreError::NotFound) => json_error("404 Not Found", "That classified is not on this node.", origin),
            Err(err) => error_response(err, origin),
        });
    }
    let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
    if method == "POST" && list {
        return Some(match parsed.map(|value| node.create_classified(&value)) {
            Some(Ok(value)) => json_body("200 OK", &value, origin),
            Some(Err(err)) => error_response(err, origin),
            None => json_error("400 Bad Request", "The classified could not be read.", origin),
        });
    }
    Some(match parsed.map(|value| node.contact_classified(contact.unwrap(), &value)) {
        Some(Ok(value)) => json_body("200 OK", &value, origin),
        Some(Err(StoreError::NotFound)) => json_error("404 Not Found", "That classified is not on this node.", origin),
        Some(Err(err)) => error_response(err, origin),
        None => json_error("400 Bad Request", "The message could not be read.", origin),
    })
}

fn contact_routes(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    let message = path.strip_prefix("/contact/").and_then(|rest| rest.strip_suffix("/message")).filter(|id| !id.is_empty() && !id.contains('/'));
    let one = path.strip_prefix("/contact/").filter(|rest| !rest.is_empty() && !rest.contains('/') && message.is_none());
    let list = path == "/contact";
    let known = matches!((method, list, one.is_some(), message.is_some()), ("GET", true, _, _) | ("GET", _, true, _) | ("POST", _, _, true));
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    if method == "GET" && list {
        return Some(json_body("200 OK", &node.contact_threads_value(), origin));
    }
    if method == "GET" {
        return Some(match node.contact_thread_value(one.unwrap()) {
            Some(value) => json_body("200 OK", &value, origin),
            None => json_error("404 Not Found", "That contact thread is not on this node.", origin),
        });
    }
    let parsed = if body.len() > MAX_JSON { None } else { serde_json::from_slice::<Value>(body).ok() };
    Some(match parsed.map(|value| node.contact_reply(message.unwrap(), &value)) {
        Some(Ok(value)) => json_body("200 OK", &value, origin),
        Some(Err(StoreError::NotFound)) => json_error("404 Not Found", "That contact thread is not on this node.", origin),
        Some(Err(err)) => error_response(err, origin),
        None => json_error("400 Bad Request", "The message could not be read.", origin),
    })
}

fn request_query(head: &str) -> &str {
    let target = head.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("");
    target.split_once('?').map(|(_, query)| query).unwrap_or("")
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name && !value.is_empty()).then_some(value)
    })
}
