//! What a selling node shows other nodes, and the follow list.
//!
//! `GET /public/store` and `GET /public/media/{hash}` need no token: other
//! nodes call them to copy this store ([`crate::replicas`]), and a buyer's
//! page may read the same bytes. They answer only while the user chose to
//! sell, and only with records this node signed and the images those records
//! name. The loopback listener still refuses a request that carries `Origin`
//! ([`refuse_pages`]). The sale onion and the https gateway strip `Origin`
//! before forwarding, and the answer allows any page to read it, without
//! cookies. `GET /follows`, `POST /follows`, and `DELETE /follows` change
//! which stores this node copies; paired loopback callers only.

use serde_json::{json, Value};

use crate::escrow_mail::hex;
use crate::http::{json_body, json_error, local_guard, public_json, public_response, Client};
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
        ("GET", "/public/catalog") => Some(refuse_pages(head).unwrap_or_else(|| public_catalog(node))),
        ("GET", "/public/admission-index") => Some(refuse_pages(head).unwrap_or_else(|| admission_index(node))),
        ("GET", "/public/admission-addresses") => Some(refuse_pages(head).unwrap_or_else(|| admission_addresses(node))),
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

/// The directory's signed admission snapshot and nothing else.
fn admission_index(node: &Node) -> Vec<u8> {
    match node.public_admission_record() {
        Some(bytes) => public_json("200 OK", &json!({ "record": hex(&bytes) })),
        None => public_error("404 Not Found", "This node has no admission snapshot."),
    }
}

/// Store key and onion or https address for ids the snapshot names. No listing body.
fn admission_addresses(node: &Node) -> Vec<u8> {
    let stores: Vec<Value> = node
        .public_admission_addresses()
        .iter()
        .map(|(id, url)| json!({ "storeId": id, "url": url }))
        .collect();
    public_json("200 OK", &json!({ "stores": stores }))
}

fn public_error(status: &str, message: &str) -> Vec<u8> {
    public_json(status, &json!({ "error": message }))
}

fn store(node: &Node) -> Vec<u8> {
    let Some(record) = node.own_record().filter(|_| selling(node)) else {
        return public_error("404 Not Found", NOT_SELLING);
    };
    let encoded = record.encode().map(|bytes| hex(&bytes));
    let listings: Result<Vec<String>, _> =
        node.own_listings().entries.iter().map(|entry| entry.listing.encode().map(|bytes| hex(&bytes))).collect();
    match (encoded, listings) {
        // The accepted moderators' signed profiles, so a node that copies the store can show and verify them.
        (Ok(store_record), Ok(listings)) => {
            let mut value = json!({ "store": store_record, "listings": listings, "moderators": node.accepted_profiles() });
            if let Some(bytes) = node.own_catalog_index_record() {
                value["catalogIndex"] = json!(hex(&bytes));
            }
            public_json("200 OK", &value)
        }
        _ => public_error("500 Internal Server Error", "The store could not be read."),
    }
}

/// Signed store + listing records for every store in this node's catalog.
/// Each entry is verified client-side — zero extra trust in the moderator.
fn public_catalog(node: &Node) -> Vec<u8> {
    let catalog = node.catalog();
    let mut stores = Vec::new();
    for s in &catalog.stores {
        let Ok(store_hex) = s.encode().map(|bytes| hex(&bytes)) else { continue };
        let listings: Vec<String> = catalog
            .listings
            .iter()
            .filter(|listing| listing.store_id == s.id)
            .filter_map(|listing| listing.encode().ok().map(|bytes| hex(&bytes)))
            .collect();
        stores.push(json!({ "store": store_hex, "listings": listings }));
    }
    public_json("200 OK", &json!({ "stores": stores }))
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

/// Image hashes from all stores and listings in this node's catalog
/// (own store, demo stores, followed stores, and admitted stores).
fn catalog_images(node: &Node) -> Vec<[u8; 32]> {
    let catalog = node.catalog();
    let mut out = Vec::new();
    for store in &catalog.stores {
        if let Some(details) = &store.details {
            out.extend(details.profile.avatar);
            out.extend(details.profile.banner);
        }
    }
    for listing in &catalog.listings {
        out.extend(listing.details.iter().flat_map(|details| details.images.iter().copied()));
    }
    out
}

fn image(node: &Node, hash: &str) -> Vec<u8> {
    let parsed = crate::stores::unhex32(hash.trim_end_matches('/'));
    let found = parsed
        .filter(|hash| {
            (selling(node) && served_images(node).contains(hash)) || catalog_images(node).contains(hash)
        })
        .and_then(|hash| node.media.get(&hash));
    match found {
        Some((content_type, bytes)) => public_response("200 OK", content_type, &bytes),
        None => public_error("404 Not Found", "That image is not on this store."),
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

#[cfg(test)]
mod tests {
    use openbazaar_protocol::{
        secret_from_label, xonly, AdmissionIndex, ContentLabel, ContentRating, DirectoryTerms, DirectPay, Listing, ListingKind, SeedDocument,
        Store, StoreDetails, StoreProfile,
    };

    use crate::harness::{self, MockChain, TempDir};
    use crate::network::NetworkMode;
    use crate::seller_store_tests::peer_request;

    #[test]
    fn the_admission_routes_return_the_signed_snapshot_and_addresses_only() {
        let chain = MockChain::new();
        let dir = TempDir::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let (status, _) = peer_request(&node, "/public/admission-index");
        assert!(status.contains("404"), "{status}");

        let secret = secret_from_label("public-admission");
        let key = xonly(&secret);
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion".to_string();
        let store = Store::sign(
            "Cedar Workshop",
            "Maine",
            DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            ContentRating::General,
            StoreDetails {
                mailbox_key: [4u8; 32],
                onion: onion.clone(),
                moderators: vec![],
                profile: StoreProfile { created_on: 20261123, ..StoreProfile::default() },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let listing = Listing::sign(
            store.id,
            "Hidden spoon",
            "Home",
            1000,
            "Maine",
            "A small spoon.",
            store.name.clone(),
            3,
            20261123,
            false,
            ListingKind::Digital,
            "",
            0,
            0,
            ContentLabel::GENERAL,
            &secret,
        )
        .unwrap();
        let head = SeedDocument::sign(3, vec![onion.clone()], DirectoryTerms::standard(key), None, &secret).unwrap();
        let snapshot = AdmissionIndex::sign(1, 20261123, 3, vec![*store.id.as_bytes()], &secret).unwrap();
        {
            let mut replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_document(&head.encode().unwrap(), &key).unwrap();
            replica.accept_admission_index(&snapshot.encode().unwrap(), &key).unwrap();
            replica.accept_store(&store.encode().unwrap()).unwrap();
            replica.accept_listing(&listing.encode().unwrap()).unwrap();
        }

        let (status, body) = peer_request(&node, "/public/admission-index");
        assert!(status.contains("200"), "{status}");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 1, "{value}");
        let record = crate::escrow_mail::unhex(value["record"].as_str().unwrap()).unwrap();
        let decoded = AdmissionIndex::decode(&record).unwrap();
        decoded.verify_signer(&key).unwrap();
        assert_eq!(decoded.stores, vec![*store.id.as_bytes()]);
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("Hidden spoon"), "{text}");

        let (status, body) = peer_request(&node, "/public/admission-addresses");
        assert!(status.contains("200"), "{status}");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["stores"]);
        let row = &value["stores"][0];
        assert_eq!(row.as_object().unwrap().len(), 2, "{row}");
        assert_eq!(row["url"], format!("http://{onion}"));
        assert_eq!(row["storeId"].as_str().unwrap().len(), 64);
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("Hidden spoon") && !text.contains("Cedar"), "{text}");
    }

    #[test]
    fn public_catalog_returns_signed_records_for_catalog_stores() {
        let chain = MockChain::new();
        let dir = TempDir::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());

        let secret = secret_from_label("public-catalog");
        let store = Store::sign(
            "Catalog Workshop",
            "Maine",
            DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            ContentRating::General,
            StoreDetails {
                mailbox_key: [5u8; 32],
                onion: "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion".to_string(),
                moderators: vec![],
                profile: StoreProfile { created_on: 20261127, ..StoreProfile::default() },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let listing = Listing::sign(
            store.id,
            "Public spoon",
            "Home",
            2000,
            "Maine",
            "A catalog spoon.",
            store.name.clone(),
            3,
            20261127,
            false,
            ListingKind::Digital,
            "",
            0,
            0,
            ContentLabel::GENERAL,
            &secret,
        )
        .unwrap();
        {
            let mut catalog = node.catalog_mut();
            catalog.stores.push(store.clone());
            catalog.listings.push(listing.clone());
        }

        let (status, body) = peer_request(&node, "/public/catalog");
        assert!(status.contains("200"), "{status}");
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let stores = value["stores"].as_array().unwrap();
        assert!(stores.len() >= 1, "at least the added store: {value}");
        let found = stores.iter().find(|entry| {
            let store_hex = entry["store"].as_str().unwrap_or("");
            let decoded = crate::escrow_mail::unhex(store_hex).ok();
            decoded.and_then(|bytes| openbazaar_protocol::Store::decode(&bytes).ok()).is_some_and(|s| s.name == "Catalog Workshop")
        });
        assert!(found.is_some(), "the added store should appear in /public/catalog: {value}");
        let entry = found.unwrap();
        let listings_arr = entry["listings"].as_array().unwrap();
        assert_eq!(listings_arr.len(), 1, "one listing for the store");
        let listing_hex = listings_arr[0].as_str().unwrap();
        let listing_bytes = crate::escrow_mail::unhex(listing_hex).unwrap();
        let decoded_listing = openbazaar_protocol::Listing::decode(&listing_bytes).unwrap();
        decoded_listing.verify().unwrap();
        assert_eq!(decoded_listing.title, "Public spoon");

        // Private routes are not exposed through /public/*.
        // /wallet requires pairing and returns 403 (no token) — that is correct.
        let (wallet_status, _) = peer_request(&node, "/wallet");
        assert!(!wallet_status.contains("200"), "wallet must not be open: {wallet_status}");
    }
}
