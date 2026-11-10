//! What a selling node shows other nodes, and the follow list.
//!
//! `GET /public/store` and `GET /public/media/{hash}` need no token: other
//! nodes call them to copy this store ([`crate::replicas`]). They answer only
//! while the user chose to sell, only with records this node signed and the
//! images those records name, and never to a browser page (nodes send no
//! `Origin`). `GET /follows`, `POST /follows`, and `DELETE /follows` change
//! which stores this node copies; paired loopback callers only.

use serde_json::{json, Value};

use crate::escrow_mail::hex;
use crate::http::{http_response_cors, json_body, json_error, local_guard, Client};
use crate::http_parse::has_header;
use crate::store_profile::Role;
use crate::Node;

const NOT_SELLING: &str = "This node is not selling.";

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Option<Vec<u8>> {
    match (method, path) {
        ("GET", "/public/store") => Some(refuse_pages(head).unwrap_or_else(|| store(node))),
        ("GET", path) if path.starts_with("/public/media/") => {
            let hash = &path["/public/media/".len()..];
            Some(refuse_pages(head).unwrap_or_else(|| image(node, hash)))
        }
        ("GET" | "POST" | "DELETE", "/follows") => Some(follows(node, client, head, method, body, origin)),
        _ => None,
    }
}

pub(crate) fn refuse_pages(head: &str) -> Option<Vec<u8>> {
    has_header(head, "origin").then(|| json_error("403 Forbidden", "Stores are copied by nodes, not pages.", None))
}

fn selling(node: &Node) -> bool {
    node.role() == Some(Role::Seller)
}

fn store(node: &Node) -> Vec<u8> {
    let Some(record) = node.own_record().filter(|_| selling(node)) else {
        return json_error("404 Not Found", NOT_SELLING, None);
    };
    let encoded = record.encode().map(|bytes| hex(&bytes));
    let listings: Result<Vec<String>, _> =
        node.own_listings().entries.iter().map(|entry| entry.listing.encode().map(|bytes| hex(&bytes))).collect();
    match (encoded, listings) {
        // The accepted moderators' signed profiles, so a node that copies the store can show and verify them.
        (Ok(store), Ok(listings)) => {
            json_body("200 OK", &json!({ "store": store, "listings": listings, "moderators": node.accepted_profiles() }), None)
        }
        _ => json_error("500 Internal Server Error", "The store could not be read.", None),
    }
}

/// Images the own store's record and current listings name.
fn served_images(node: &Node) -> Vec<[u8; 32]> {
    let mut out = Vec::new();
    if let Some(details) = node.own_record().and_then(|store| store.details) {
        out.extend(details.profile.avatar);
        out.extend(details.profile.banner);
    }
    for entry in &node.own_listings().entries {
        out.extend(entry.listing.details.iter().flat_map(|details| details.images.iter().copied()));
    }
    out
}

fn image(node: &Node, hash: &str) -> Vec<u8> {
    let parsed = crate::stores::unhex32(hash.trim_end_matches('/'));
    let found = parsed
        .filter(|hash| selling(node) && served_images(node).contains(hash))
        .and_then(|hash| node.media.get(&hash));
    match found {
        Some((content_type, bytes)) => http_response_cors("200 OK", content_type, &bytes, None),
        None => json_error("404 Not Found", "That image is not on this store.", None),
    }
}

fn follows(node: &Node, client: &Client, head: &str, method: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    if method == "GET" {
        return json_body("200 OK", &node.follows_value(), origin);
    }
    let url = serde_json::from_slice::<Value>(body).ok().and_then(|value| value["url"].as_str().map(str::to_string));
    let Some(url) = url.filter(|_| body.len() <= 4096) else {
        return json_error("400 Bad Request", "Send the store node's URL.", origin);
    };
    let result = if method == "POST" { node.follow(&url) } else { node.unfollow(&url) };
    match result {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(message) => json_error("400 Bad Request", &message, origin),
    }
}
