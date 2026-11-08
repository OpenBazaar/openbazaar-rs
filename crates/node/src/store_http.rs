//! Loopback routes for selling: the storefront, its listings, and image
//! uploads. Paired loopback callers only.
//!
//! - `GET /store/profile`, `PUT /store/profile`: the storefront.
//! - `PUT /store/role`: `{"role": "buyer" | "seller"}`, chosen at onboarding.
//! - `PUT /store/vacation`: `{"away": true|false}`. Pauses the store. No note.
//! - `GET /store/listings`, `POST /store/listings`: the own store's listings.
//! - `PUT /store/listings/{id}`, `DELETE /store/listings/{id}`: by stable id.
//! - `GET /store/drafts`, `POST /store/drafts`: unpublished listings. Not signed.
//! - `PUT /store/drafts/{id}`, `DELETE /store/drafts/{id}`: by draft id.
//! - `POST /media`: one image as the raw body; answers its SHA-256.

use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::own_listings::StoreError;
use crate::store_profile::Role;
use crate::Node;

const MAX_JSON: usize = 32 * 1024;

pub(crate) fn error_response(err: StoreError, origin: Option<&str>) -> Vec<u8> {
    match err {
        StoreError::Invalid(message) => json_error("400 Bad Request", &message, origin),
        StoreError::NotFound => json_error("404 Not Found", "That listing is not in your store.", origin),
        StoreError::Locked(state) => crate::lock_http::locked_response(state, origin),
        StoreError::Failed(detail) => {
            eprintln!("ob-node: store: {detail}");
            json_error("500 Internal Server Error", "The store could not be saved.", origin)
        }
    }
}

fn result(result: Result<Value, StoreError>, origin: Option<&str>) -> Vec<u8> {
    match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => error_response(err, origin),
    }
}

fn read_json(body: &[u8]) -> Result<Value, StoreError> {
    if body.len() > MAX_JSON {
        return Err(StoreError::Invalid("That request is too large.".into()));
    }
    serde_json::from_slice(body).map_err(|_| StoreError::Invalid("That request could not be read.".into()))
}

/// The route for `method` and `path` under `/store/` or `POST /media`,
/// or `None` when this module has none.
pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    let listing_id = path.strip_prefix("/store/listings/");
    let draft_id = path.strip_prefix("/store/drafts/");
    let known = matches!(
        (method, path),
        ("GET" | "PUT", "/store/profile")
            | ("PUT", "/store/role")
            | ("PUT", "/store/vacation")
            | ("GET" | "POST", "/store/listings")
            | ("GET" | "POST", "/store/drafts")
            | ("POST", "/media")
    ) || (listing_id.is_some() && matches!(method, "PUT" | "DELETE"))
        || (draft_id.is_some() && matches!(method, "PUT" | "DELETE"));
    if !known {
        return None;
    }
    if let Err(response) = local_guard(node, client, head, origin) {
        return Some(response);
    }
    Some(match (method, path) {
        ("GET", "/store/profile") => result(node.profile_value(), origin),
        ("PUT", "/store/profile") => result(read_json(body).and_then(|value| node.update_profile(&value)), origin),
        ("PUT", "/store/vacation") => result(read_json(body).and_then(|value| node.set_vacation(&value)), origin),
        ("PUT", "/store/role") => result(
            read_json(body).and_then(|value| {
                let role = value["role"].as_str().and_then(Role::parse);
                node.set_role(role.ok_or_else(|| StoreError::Invalid("Choose buyer or seller.".into()))?)
            }),
            origin,
        ),
        ("GET", "/store/listings") => result(node.own_listings_value(), origin),
        ("POST", "/store/listings") => result(read_json(body).and_then(|value| node.create_own_listing(&value)), origin),
        ("GET", "/store/drafts") => result(node.listing_drafts_value(), origin),
        ("POST", "/store/drafts") => result(read_json(body).and_then(|value| node.save_listing_draft(None, &value)), origin),
        ("PUT", path) if path.starts_with("/store/drafts/") => {
            result(read_json(body).and_then(|value| node.save_listing_draft(draft_id, &value)), origin)
        }
        ("DELETE", path) if path.starts_with("/store/drafts/") => {
            result(node.delete_listing_draft(draft_id.unwrap_or_default()).map(|()| json!({ "deleted": true })), origin)
        }
        ("POST", "/media") => upload(node, body, origin),
        ("PUT", _) => {
            let id = listing_id.unwrap_or_default();
            result(read_json(body).and_then(|value| node.update_own_listing(id, &value)), origin)
        }
        _ => result(node.delete_own_listing(listing_id.unwrap_or_default()).map(|()| json!({ "deleted": true })), origin),
    })
}

fn upload(node: &Node, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    match node.media.put(body) {
        Ok((hash, kind)) => json_body("200 OK", &json!({ "hash": crate::escrow_mail::hex(&hash), "contentType": kind }), origin),
        Err(message) => json_error("400 Bad Request", &message, origin),
    }
}
