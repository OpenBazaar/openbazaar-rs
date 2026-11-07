use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use openbazaar_pay::{Bolt12Offer, SilentPaymentCode};
use openbazaar_protocol::fixture_store_keys;
use serde_json::{json, Value};
use tormail::{open_order, SealedLetter, StoreSecret};

use crate::http::{dispatch, respond, Client};
use crate::{load_catalog, NetworkMode, Node, NodeConfig, ESCROW_SPEND_FEE_SATS};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ob-node-{nanos}-{}-{n}", std::process::id()));
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn node() -> (TempDir, Node) {
    let dir = TempDir::new();
    let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
    let node = Node::open(&dir.0, catalog).unwrap();
    (dir, node)
}

fn hash_of(node: &Node, title: &str) -> String {
    node.catalog()
        .listings
        .iter()
        .find(|listing| listing.title == title)
        .unwrap()
        .content_hash()
        .unwrap()
        .to_hex()
}

fn status_and_body(response: &[u8]) -> (String, String) {
    let text = String::from_utf8(response.to_vec()).unwrap();
    let (header, body) = text.split_once("\r\n\r\n").unwrap();
    let status = header.lines().next().unwrap().to_string();
    assert!(header.contains("Access-Control-Allow-Origin: http://127.0.0.1:4173"));
    (status, body.to_string())
}

fn post(node: &Node, json_body: &str) -> Vec<u8> {
    format!(
        "POST /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nOrigin: http://127.0.0.1:4173\r\nX-Ob-Pair: {}\r\n\r\n{json_body}",
        json_body.len(),
        node.pairing_token(),
    )
    .into_bytes()
}

fn open_stored(node: &Node, letter_id: &str) -> tormail::Order {
    let path = node.mailbox_path().join(format!("{letter_id}.letter"));
    let bytes = fs::read(&path).unwrap();
    let letter = SealedLetter::from_bytes(&bytes).unwrap();
    let names: Vec<String> = node.catalog().stores.iter().map(|store| store.name.clone()).collect();
    names
        .iter()
        .find_map(|name| open_order(&letter, &node.seller_mail_secret(name)?).ok())
        .expect("a demo store's mailbox key opens the letter")
        .body
}

#[test]
fn rates_stay_optional_when_the_price_source_cannot_be_reached() {
    let (_dir, node) = node();
    let response = dispatch(
        &node,
        b"GET /rates HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\n\r\n",
    );
    let (status, body) = status_and_body(&response);
    assert!(status.contains("200 OK"));
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["base"], "BTC");
    let currencies = value["currencies"].as_object().unwrap();
    for code in ["USD", "EUR", "GBP", "JPY"] {
        if let Some(price) = currencies.get(code) {
            let price = price.as_f64().unwrap();
            assert!(price.is_finite() && price > 0.0);
        }
    }
}

#[test]
fn rejects_an_unknown_listing_and_a_missing_ship_to() {
    let (_dir, node) = node();
    let unknown = "ab".repeat(32);
    let response = dispatch(
        &node,
        &post(
            &node,
            &json!({"listingHash": unknown, "quantity": 1, "settlement": "direct"}).to_string(),
        ),
    );
    let (status, body) = status_and_body(&response);
    assert!(status.contains("404"));
    assert!(body.contains("That listing is not in this shop."));
    assert!(node.mailbox.list().unwrap().is_empty());

    let book = hash_of(&node, "Paperback novel");
    let missing = json!({
        "listingHash": book,
        "quantity": 1,
        "settlement": "direct",
        "shipTo": {
            "name": "Ada Brooks",
            "street": "",
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202"
        }
    });
    let response = dispatch(&node, &post(&node, &missing.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("400"));
    assert!(body.contains("ship-to"));
    assert!(node.mailbox.list().unwrap().is_empty());
}

#[test]
fn seals_the_ship_to_into_the_mailbox_and_keeps_it_out_of_the_catalog() {
    let (_dir, node) = node();
    let street = "14 Larch Lane";
    let book = hash_of(&node, "Paperback novel");
    let order = json!({
        "listingHash": book,
        "quantity": 2,
        "settlement": "direct",
        "shipTo": {
            "name": "Ada Brooks",
            "street": street,
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202"
        }
    });
    let response = dispatch(&node, &post(&node, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("200"));
    let receipt: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(receipt["sealed"], true);
    assert_eq!(receipt["settlement"], "direct");
    assert_eq!(receipt["totalSats"], 36_000);
    assert!(!body.contains(street));

    let lightning = receipt["payment"]["lightning"].as_str().unwrap();
    let onchain = receipt["payment"]["onchain"].as_str().unwrap();
    let code = receipt["payment"]["silentPaymentCode"].as_str().unwrap();
    assert!(lightning.starts_with("lno1"));
    assert!(onchain.starts_with("tb1p"), "{onchain}");
    assert_eq!(receipt["payment"]["address"], onchain);
    assert_eq!(receipt["payment"]["amountSats"], 36_000);
    assert_eq!(
        receipt["payment"]["bip21"],
        format!("bitcoin:{onchain}?amount=0.00036")
    );
    assert_eq!(receipt["network"], "testnet4");
    assert_eq!(receipt["status"], "Awaiting payment");
    let offer = Bolt12Offer::parse(lightning).unwrap();
    assert_eq!(offer.price().to_sat(), 36_000);
    assert!(offer.is_for(bitcoin::Network::Testnet4));
    assert!(code.starts_with("tsp1"));
    assert!(SilentPaymentCode::parse(code).unwrap().is_for(bitcoin::Network::Testnet4));

    let letter_id = receipt["letterId"].as_str().unwrap();
    let stored = fs::read(node.mailbox_path().join(format!("{letter_id}.letter"))).unwrap();
    assert!(
        !stored
            .windows(street.len())
            .any(|window| window == street.as_bytes()),
        "the mailbox file is ciphertext"
    );
    let opened = open_stored(&node, letter_id);
    assert!(opened.ship_to.contains(street));
    assert_eq!(opened.quantity, 2);
    assert_eq!(
        opened.options.get("settlement").map(String::as_str),
        Some("direct")
    );

    let catalog = dispatch(&node, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (status, body) = status_and_body(&catalog);
    assert!(status.contains("200"));
    assert!(body.contains("Paperback novel"));
    assert!(body.contains("June Pell"));
    assert!(!body.contains(street));
    assert!(!body.contains("Ada Brooks"));
}

#[test]
fn moderated_order_records_the_buyer_side_and_asks_the_vendor_node() {
    let (_dir, offline) = node();
    let note = "DELIVERY-NOTE-7c2e-reply-to-ada";
    let recording = hash_of(&offline, "Field recordings");
    let order = json!({
        "listingHash": recording,
        "quantity": 1,
        "settlement": "escrow",
        "deliveryNote": note
    });
    let response = dispatch(&offline, &post(&offline, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("400"), "no vendor node is set up: {body}");
    assert!(offline.mailbox.list().unwrap().is_empty());

    let dir = TempDir::new();
    let mut config = NodeConfig::offline(NetworkMode::Testnet4);
    config.escrow = crate::harness::unreachable_escrow();
    let node = Node::open_with(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap(), config).unwrap();
    let response = dispatch(&node, &post(&node, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("423"), "escrow checkout derives a signing key: {body}");
    assert!(node.mailbox.list().unwrap().is_empty(), "nothing is sealed while locked");
    crate::harness::unlock(&node);
    let response = dispatch(&node, &post(&node, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("200"), "{body}");
    let receipt: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(receipt["sealed"], true);
    assert_eq!(receipt["settlement"], "escrow");
    assert_eq!(receipt["escrow"]["fundingSats"], 4_000 + ESCROW_SPEND_FEE_SATS);
    assert_eq!(receipt["escrow"]["signaturesRequired"], 2);
    assert_eq!(receipt["escrow"]["stage"], "requested");
    assert!(receipt["escrow"]["buyerPayout"].as_str().unwrap().starts_with("tb1p"));
    assert!(receipt["escrow"].get("address").is_none(), "the address comes with the vendor's terms");
    assert!(receipt["escrow"]["error"].is_string(), "the vendor node could not be reached");
    assert_eq!(receipt["escrow"]["refundHeld"], false);
    assert_eq!(receipt["escrow"]["safeToSend"], false);
    assert_eq!(receipt["status"], "Awaiting vendor terms");
    assert!(!body.contains(note));
    assert!(receipt.get("payment").is_none());

    let opened = open_stored(&node, receipt["letterId"].as_str().unwrap());
    assert_eq!(opened.ship_to, note);
    assert_eq!(
        opened.options.get("settlement").map(String::as_str),
        Some("escrow")
    );

    let catalog = dispatch(&node, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (_, body) = status_and_body(&catalog);
    assert!(!body.contains(note));
}

#[test]
fn options_preflight_allows_the_shop_origin() {
    let (_dir, node) = node();
    let response = dispatch(
        &node,
        b"OPTIONS /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: content-type\r\n\r\n",
    );
    let text = String::from_utf8(response).unwrap();
    let header = text.split_once("\r\n\r\n").unwrap().0;
    assert!(header.starts_with("HTTP/1.1 204"));
    assert!(header.contains("Access-Control-Allow-Origin: http://127.0.0.1:4173"));
    assert!(header.contains("Access-Control-Allow-Headers: Content-Type"));
    assert!(header.contains("X-Ob-Pair"));
}

#[test]
fn pairing_token_is_stable_and_orders_require_it() {
    let dir = TempDir::new();
    let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
    let first = Node::open(&dir.0, catalog.clone()).unwrap();
    let token = first.pairing_token().to_string();
    assert!(first.fresh_pairing_token().is_some());
    let second = Node::open(&dir.0, catalog).unwrap();
    assert!(second.fresh_pairing_token().is_none());
    assert_eq!(second.pairing_token(), token);

    let mut code = String::new();
    first.pairing_codes.attempt(None, std::time::Instant::now(), |printed| code = printed.to_string());
    let request = format!(r#"{{"code":"{code}"}}"#);
    let paired = respond(
        &first,
        format!(
            "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\nContent-Length: {}\r\n\r\n{request}",
            request.len()
        )
        .as_bytes(),
        Client {
            loopback: true,
            port: 8787,
        },
    );
    let (status, body) = status_and_body(&paired);
    assert!(status.contains("200"));
    assert!(body.contains(&token));

    let remote = respond(
        &first,
        b"POST /pair HTTP/1.1\r\nHost: 203.0.113.9:8787\r\nContent-Length: 2\r\n\r\n{}",
        Client {
            loopback: false,
            port: 8787,
        },
    );
    let text = String::from_utf8(remote).unwrap();
    assert!(text.contains("421"));
    assert!(!text.contains(&token));

    let book = hash_of(&first, "Paperback novel");
    let order = json!({
        "listingHash": book,
        "quantity": 1,
        "settlement": "direct",
        "shipTo": {
            "name": "Ada Brooks",
            "street": "14 Larch Lane",
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202"
        }
    })
    .to_string();
    let missing = format!(
        "POST /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{order}",
        order.len()
    );
    let response = dispatch(&first, missing.as_bytes());
    let (status, body) = status_and_body(&response);
    assert!(status.contains("401"));
    assert!(body.contains("not paired"));
    assert!(first.mailbox.list().unwrap().is_empty());
}

#[test]
fn cors_allows_the_shop_and_desktop_origins_only() {
    let (_dir, node) = node();
    let own = respond(
        &node,
        b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\n\r\n",
        Client {
            loopback: true,
            port: 8787,
        },
    );
    let text = String::from_utf8(own).unwrap();
    assert!(text.starts_with("HTTP/1.1 403"), "the node serves no page of its own");
    assert!(!text.contains("Access-Control-Allow-Origin"));

    let foreign = respond(
        &node,
        b"GET /catalog HTTP/1.1\r\nHost: evil.example\r\nOrigin: https://evil.example\r\n\r\n",
        Client {
            loopback: false,
            port: 8787,
        },
    );
    let text = String::from_utf8(foreign).unwrap();
    let header = text.split_once("\r\n\r\n").unwrap().0;
    assert!(!header.contains("Access-Control-Allow-Origin"));
    assert!(!text.contains(node.pairing_token()));
}

#[test]
fn desktop_webview_origins_are_allowed_but_get_no_token() {
    let (_dir, node) = node();
    for origin in ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"] {
        for request in [
            format!("GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:49152\r\nOrigin: {origin}\r\n\r\n"),
            format!("GET /pair HTTP/1.1\r\nHost: 127.0.0.1:49152\r\nOrigin: {origin}\r\n\r\n"),
        ] {
            let response = respond(
                &node,
                request.as_bytes(),
                Client {
                    loopback: true,
                    port: 49152,
                },
            );
            let text = String::from_utf8(response).unwrap();
            assert!(text.contains(&format!("Access-Control-Allow-Origin: {origin}\r\n")));
            assert!(!text.contains(node.pairing_token()));
        }
    }
    let lookalike = respond(
        &node,
        b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: tauri://localhost.evil.example\r\n\r\n",
        Client {
            loopback: true,
            port: 8787,
        },
    );
    let text = String::from_utf8(lookalike).unwrap();
    assert!(!text.split_once("\r\n\r\n").unwrap().0.contains("Access-Control-Allow-Origin"));
}

fn authed(node: &Node, request: &str) -> Vec<u8> {
    let (method_path, rest) = request.split_once("\r\n").unwrap();
    format!(
        "{method_path}\r\nX-Ob-Pair: {}\r\n{rest}",
        node.pairing_token()
    )
    .into_bytes()
}

#[test]
fn address_book_is_encrypted_and_stays_off_the_catalog() {
    let (dir, node) = node();
    let street = "41 Hidden Wharf Lane";
    let body = json!({
        "label": "Home",
        "name": "Ada Brooks",
        "street": street,
        "city": "Portland",
        "region": "Oregon",
        "postalCode": "97202",
        "country": "United States"
    });
    let created = dispatch(&node, &authed(&node, &format!(
        "POST /addresses HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.to_string().len()
    )));
    let (status, text) = status_and_body(&created);
    assert!(status.contains("200"), "{text}");
    let saved: Value = serde_json::from_str(&text).unwrap();
    let id = saved["id"].as_str().unwrap();
    assert_eq!(saved["default"], true);

    let listed = dispatch(&node, &authed(&node, "GET /addresses HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
    let (_, list_body) = status_and_body(&listed);
    assert!(list_body.contains(street));
    assert!(list_body.contains("Home"));

    let edited = json!({
        "label": "Office",
        "name": "Ada Brooks",
        "street": street,
        "city": "Salem",
        "region": "Oregon",
        "postalCode": "97301",
        "country": "United States",
        "default": true
    });
    let updated = dispatch(&node, &authed(&node, &format!(
        "PUT /addresses/{id} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{edited}",
        edited.to_string().len()
    )));
    let (status, text) = status_and_body(&updated);
    assert!(status.contains("200"), "{text}");
    assert!(text.contains("Salem"));
    assert!(text.contains("Office"));

    let extra = json!({
        "label": "Pickup point",
        "name": "Ada Brooks",
        "street": "9 Dock Street",
        "city": "Astoria",
        "region": "Oregon",
        "postalCode": "97103",
        "country": "United States",
        "phone": "555-0100"
    });
    let rejected = dispatch(&node, &authed(&node, &format!(
        "POST /addresses HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{extra}",
        extra.to_string().len()
    )));
    let (status, text) = status_and_body(&rejected);
    assert!(status.contains("400"), "{text}");

    let bare = dispatch(&node, b"GET /addresses HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (status, _) = status_and_body(&bare);
    assert!(status.contains("403"));
    let remote = respond(
        &node,
        &authed(&node, "GET /addresses HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
        Client { loopback: false, port: 8787 },
    );
    assert!(String::from_utf8(remote).unwrap().contains("403"));

    let stored = fs::read(dir.0.join("addresses.obv")).unwrap();
    let stored_text = String::from_utf8_lossy(&stored);
    assert!(!stored_text.contains(street));
    assert!(!stored_text.contains("Ada Brooks"));

    let catalog = dispatch(&node, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (_, catalog_body) = status_and_body(&catalog);
    assert!(!catalog_body.contains(street));
    assert!(!catalog_body.contains("Hidden Wharf"));

    let removed = dispatch(&node, &authed(&node, &format!(
        "DELETE /addresses/{id} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    )));
    let (status, _) = status_and_body(&removed);
    assert!(status.contains("200"));
}

#[test]
fn buyer_orders_are_encrypted_and_stay_off_the_catalog() {
    let (dir, node) = node();
    let street = "41 Hidden Wharf Lane";
    let book = hash_of(&node, "Paperback novel");
    let order = json!({
        "listingHash": book,
        "quantity": 1,
        "settlement": "direct",
        "addressLabel": "Home",
        "shipTo": {
            "name": "Ada Brooks",
            "street": street,
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202",
            "country": "United States"
        }
    });
    let response = dispatch(&node, &post(&node, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("200"), "{body}");
    let receipt: Value = serde_json::from_str(&body).unwrap();
    let id = receipt["orderId"].as_str().unwrap();
    assert_eq!(receipt["addressLabel"], "Home");
    assert!(!body.contains(street));

    let missing = dispatch(&node, b"GET /orders HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (status, _) = status_and_body(&missing);
    assert!(status.contains("403"));

    let listed = dispatch(&node, &authed(&node, "GET /orders HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
    let (status, list_body) = status_and_body(&listed);
    assert!(status.contains("200"), "{list_body}");
    let list: Value = serde_json::from_str(&list_body).unwrap();
    assert_eq!(list["orders"][0]["id"], id);
    assert_eq!(list["orders"][0]["status"], "Awaiting payment");
    assert_eq!(list["orders"][0]["seller"], "Marlow Goods");
    assert_eq!(list["orders"][0]["addressLabel"], "Home");
    assert!(!list_body.contains(street));
    assert!(list["orders"][0]["lines"][0]["title"].as_str().unwrap() == "Paperback novel");

    let one = dispatch(&node, &authed(&node, &format!(
        "GET /orders/{id} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    )));
    let (status, one_body) = status_and_body(&one);
    assert!(status.contains("200"), "{one_body}");
    assert!(one_body.contains("lno1"));
    assert!(!one_body.contains(street));

    let stored = fs::read(dir.0.join("orders.obv")).unwrap();
    let stored_text = String::from_utf8_lossy(&stored);
    assert!(!stored_text.contains(street));
    assert!(!stored_text.contains("Paperback novel"));
    assert!(!stored_text.contains("Ada Brooks"));

    let catalog = dispatch(&node, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let (_, catalog_body) = status_and_body(&catalog);
    assert!(!catalog_body.contains(id));
    assert!(!catalog_body.contains(street));
    assert!(!catalog_body.contains("Awaiting payment"));
}

#[test]
fn rejects_a_client_supplied_destination() {
    let (_dir, node) = node();
    let book = hash_of(&node, "Paperback novel");
    let order = json!({
        "listingHash": book,
        "quantity": 1,
        "settlement": "direct",
        "destination": "http://203.0.113.9/orders",
        "peer": "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst.onion",
        "shipTo": {
            "name": "Ada Brooks",
            "street": "14 Larch Lane",
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202"
        }
    });
    let response = dispatch(&node, &post(&node, &order.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("400"));
    assert!(body.contains("destination"));
    assert!(!body.contains("203.0.113.9"));
    assert!(!body.contains(".onion"));
    assert!(node.mailbox.list().unwrap().is_empty());

    let note = json!({
        "listingHash": hash_of(&node, "Field recordings"),
        "quantity": 1,
        "settlement": "direct",
        "deliveryNote": "reply at shop.onion"
    });
    let response = dispatch(&node, &post(&node, &note.to_string()));
    let (status, body) = status_and_body(&response);
    assert!(status.contains("400"), "{body}");
    assert!(node.mailbox.list().unwrap().is_empty());
}

fn checkout_post(node: &Node, json_body: &str) -> Vec<u8> {
    format!(
        "POST /checkout HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nOrigin: http://127.0.0.1:4173\r\nX-Ob-Pair: {}\r\n\r\n{json_body}",
        json_body.len(),
        node.pairing_token(),
    )
    .into_bytes()
}

fn seller_secret(seller: &str) -> StoreSecret {
    StoreSecret::from_bytes(fixture_store_keys(seller).unwrap().mailbox).unwrap()
}

fn opened_hashes(order: &tormail::Order) -> Vec<String> {
    let mut hashes = vec![order
        .listing_hash
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()];
    for key in order.options.keys() {
        if key.len() == 64 && key.chars().all(|ch| ch.is_ascii_hexdigit()) {
            hashes.push(key.clone());
        }
    }
    hashes
}

#[test]
fn multi_seller_checkout_seals_one_letter_per_vendor() {
    let (_dir, node) = node();
    let book = hash_of(&node, "Paperback novel");
    let guide = hash_of(&node, "Field guide to tide pools");
    let cards = hash_of(&node, "Letterpress cards");
    let coffee = hash_of(&node, "Whole bean coffee, 12 oz");
    let body = json!({
        "addressLabel": "Home",
        "shipTo": {
            "name": "Ada Brooks",
            "street": "14 Larch Lane",
            "city": "Portland",
            "region": "Oregon",
            "postalCode": "97202"
        },
        "packages": [
            {"settlement": "direct", "lines": [{"listingHash": book, "quantity": 1}, {"listingHash": guide, "quantity": 1}]},
            {"settlement": "direct", "lines": [{"listingHash": cards, "quantity": 1}]},
            {"settlement": "direct", "lines": [{"listingHash": coffee, "quantity": 1}]}
        ]
    });
    let response = dispatch(&node, &checkout_post(&node, &body.to_string()));
    let (status, text) = status_and_body(&response);
    assert!(status.contains("200"), "{text}");
    let receipt: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(receipt["sealed"], true);
    assert_eq!(receipt["shippingSats"], 1_500);
    assert_eq!(receipt["itemsSats"], 18_000 + 12_500 + 4_800 + 9_500);
    let orders = receipt["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 3);
    assert_eq!(orders[0]["shippingSats"], 0);
    assert_eq!(orders[0]["itemsSats"], 30_500);
    assert_eq!(orders[1]["shippingSats"], 600);
    assert_eq!(orders[1]["seller"], "Hale Paper");
    assert_eq!(orders[2]["shippingSats"], 900);
    assert_eq!(orders[2]["seller"], "North Kitchen");
    assert!(orders.iter().all(|order| order["payment"]["lightning"].as_str().unwrap().starts_with("lno1")));
    assert_eq!(node.mailbox.list().unwrap().len(), 3);

    let checkout_id = receipt["checkoutId"].as_str().unwrap();
    let mut seen = Vec::new();
    for order in orders {
        let id = order["orderId"].as_str().unwrap();
        let seller = order["seller"].as_str().unwrap();
        let path = node.mailbox_path().join(format!("{id}.letter"));
        let letter = SealedLetter::from_bytes(&fs::read(path).unwrap()).unwrap();
        for other in ["Marlow Goods", "Hale Paper", "North Kitchen"] {
            if other == seller {
                continue;
            }
            assert!(open_order(&letter, &seller_secret(other)).is_err(), "{seller} opened by {other}");
        }
        let opened = open_order(&letter, &seller_secret(seller)).unwrap().body;
        assert!(opened.ship_to.contains("14 Larch Lane"));
        assert!(!opened.ship_to.contains(checkout_id));
        let expected_shipping = order["shippingSats"].as_u64().unwrap().to_string();
        assert_eq!(opened.options.get("shipping").map(String::as_str), Some(expected_shipping.as_str()));
        let hashes = opened_hashes(&opened);
        seen.push((seller.to_string(), hashes));
    }
    let marlow = &seen.iter().find(|(seller, _)| seller == "Marlow Goods").unwrap().1;
    assert!(marlow.contains(&book));
    assert!(marlow.contains(&guide));
    assert!(!marlow.contains(&cards));
    assert!(!marlow.contains(&coffee));
    let hale = &seen.iter().find(|(seller, _)| seller == "Hale Paper").unwrap().1;
    assert_eq!(hale, &vec![cards.clone()]);
    let north = &seen.iter().find(|(seller, _)| seller == "North Kitchen").unwrap().1;
    assert_eq!(north, &vec![coffee.clone()]);

    let listed = dispatch(&node, &authed(&node, "GET /orders HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
    let (_, list_body) = status_and_body(&listed);
    let list: Value = serde_json::from_str(&list_body).unwrap();
    let stored = list["orders"].as_array().unwrap();
    assert!(stored.iter().all(|order| order["checkoutId"] == checkout_id));
    assert!(stored.iter().all(|order| order["status"] == "Awaiting payment"));
}

#[test]
fn checkout_rejects_a_bad_package_before_sealing() {
    let (_dir, node) = node();
    let bare = dispatch(&node, b"POST /checkout HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}");
    let (status, _) = status_and_body(&bare);
    assert!(status.contains("403"));

    let book = hash_of(&node, "Paperback novel");
    let extra = json!({
        "phone": "555-0100",
        "packages": [{"settlement": "direct", "lines": [{"listingHash": book, "quantity": 1}]}],
        "shipTo": {"name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202"}
    });
    let response = dispatch(&node, &checkout_post(&node, &extra.to_string()));
    let (status, text) = status_and_body(&response);
    assert!(status.contains("400"), "{text}");
    assert!(node.mailbox.list().unwrap().is_empty());

    let bad = json!({
        "shipTo": {"name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202"},
        "packages": [
            {"settlement": "direct", "lines": [{"listingHash": book, "quantity": 1}]},
            {"settlement": "direct", "lines": [{"listingHash": "ab".repeat(32), "quantity": 1}]}
        ]
    });
    let response = dispatch(&node, &checkout_post(&node, &bad.to_string()));
    let (status, text) = status_and_body(&response);
    assert!(status.contains("404"), "{text}");
    assert!(node.mailbox.list().unwrap().is_empty());
}
