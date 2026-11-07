//! Store pages, store media, and the keys order letters use.

use bitcoin::hashes::{sha256, Hash};
use serde_json::{json, Value};
use tormail::{open_order, open_reply, seal_reply, SealedLetter, SenderKey};

use crate::harness::{hash_of, open_node, request, ship_to, MockChain, TempDir};
use crate::http::{dispatch, respond, Client};
use crate::network::NetworkMode;
use crate::{EscrowConfig, Node};

fn testnet() -> (TempDir, Node) {
    let dir = TempDir::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, EscrowConfig::default());
    (dir, node)
}

fn get(node: &Node, path: &str, token: Option<&str>) -> Vec<u8> {
    let pair = token.map(|token| format!("X-Ob-Pair: {token}\r\n")).unwrap_or_default();
    dispatch(node, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{pair}\r\n").as_bytes())
}

fn status(response: &[u8]) -> String {
    let text = String::from_utf8_lossy(response);
    text.lines().next().unwrap().to_string()
}

fn split(response: &[u8]) -> (String, Vec<u8>) {
    let at = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    (String::from_utf8(response[..at].to_vec()).unwrap(), response[at + 4..].to_vec())
}

#[test]
fn store_list_and_pages_are_for_the_paired_client_only() {
    let (_dir, node) = testnet();
    for path in ["/stores", &format!("/stores/{}", node.catalog().stores[0].id.to_hex())] {
        assert!(status(&get(&node, path, None)).contains("403"), "{path}");
        assert!(status(&get(&node, path, Some(&"0".repeat(64)))).contains("403"), "{path}");
        let stranger = Client { loopback: false, port: 8787 };
        let raw = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Ob-Pair: {}\r\n\r\n", node.pairing_token());
        assert!(status(&respond(&node, raw.as_bytes(), stranger)).contains("403"), "{path}");
    }
}

#[test]
fn store_list_has_the_demo_stores_and_this_nodes_own_store() {
    let (_dir, node) = testnet();
    let (status, value) = request(&node, "GET", "/stores", "");
    assert!(status.contains("200"), "{status}");
    let stores = value["stores"].as_array().unwrap();
    let names: Vec<&str> = stores.iter().map(|store| store["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Marlow Goods", "Hale Paper", "North Kitchen", "My store"]);
    for store in stores {
        assert_eq!(store["verified"], true);
        assert_eq!(store["version"], 3);
        assert_eq!(store["key"].as_str().unwrap().len(), 64);
        assert!(store["record"].as_str().unwrap().starts_with("4f425354"), "OBST");
    }
    let own: Vec<&Value> = stores.iter().filter(|store| store["own"] == true).collect();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0]["listingCount"], 0);
    assert_eq!(own[0]["key"], node.own_store.to_hex());
    let expected = bitcoin::secp256k1::Keypair::from_secret_key(
        &bitcoin::secp256k1::Secp256k1::new(),
        &node.own_signing_key().expect("the harness node is unlocked"),
    )
    .x_only_public_key()
    .0
    .serialize();
    assert_eq!(*node.own_store.as_bytes(), expected, "the own store is signed with the seed's store key");
    let marlow = &stores[0];
    assert!(marlow["listingCount"].as_u64().unwrap() > 0);
    assert_eq!(marlow["tagline"].as_str().unwrap().is_empty(), false);
    assert_eq!(marlow["avatar"].as_str().unwrap().len(), 64);
}

#[test]
fn store_page_carries_the_profile_listings_and_moderators() {
    let (_dir, node) = testnet();
    let store = node.catalog().stores[1].clone();
    let (status, value) = request(&node, "GET", &format!("/stores/{}", store.id.to_hex()), "");
    assert!(status.contains("200"), "{status}");
    let page = &value["store"];
    assert_eq!(page["name"], "Hale Paper");
    assert_eq!(page["shipsTo"], json!(["US"]));
    assert_eq!(page["createdOn"], 20260120);
    assert!(page["onion"].is_null());
    for key in ["shipping", "returns", "processingTime"] {
        assert!(!page["policies"][key].as_str().unwrap().is_empty(), "{key}");
    }
    let details = store.details.as_ref().unwrap();
    assert_eq!(page["mailboxKey"], details.mailbox_key.iter().map(|b| format!("{b:02x}")).collect::<String>());
    assert_eq!(value["moderators"], json!([{ "id": node.catalog().moderator.id.iter().map(|b| format!("{b:02x}")).collect::<String>(), "name": "June Pell" }]));
    let listings = value["listings"].as_array().unwrap();
    assert!(!listings.is_empty());
    for listing in listings {
        assert_eq!(listing["seller"], "Hale Paper");
        assert_eq!(listing["storeId"], store.id.to_hex());
        assert!(listing["record"].is_string());
    }
    let (status, _) = request(&node, "GET", &format!("/stores/{}", "ab".repeat(32)), "");
    assert!(status.contains("404"), "{status}");
    let (status, _) = request(&node, "GET", "/stores/not-a-key", "");
    assert!(status.contains("404"), "{status}");
}

#[test]
fn media_is_served_by_hash_with_its_type_and_only_when_the_hash_matches() {
    let (dir, node) = testnet();
    let (_, value) = request(&node, "GET", "/stores", "");
    let avatar = value["stores"][0]["avatar"].as_str().unwrap().to_string();
    let path = format!("/media/{avatar}");
    assert!(status(&get(&node, &path, None)).contains("403"));
    let (head, body) = split(&get(&node, &path, Some(node.pairing_token())));
    assert!(head.contains("200 OK") && head.contains("Content-Type: image/png"), "{head}");
    let hash: String = sha256::Hash::hash(&body).to_byte_array().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hash, avatar);

    let other = "cd".repeat(32);
    std::fs::create_dir_all(dir.0.join("media")).unwrap();
    std::fs::write(dir.0.join("media").join(&other), b"\x89PNG\r\n\x1a\n not the hash").unwrap();
    assert!(status(&get(&node, &format!("/media/{other}"), Some(node.pairing_token()))).contains("404"));
    assert!(status(&get(&node, "/media/../vault.key", Some(node.pairing_token()))).contains("404"));
}

/// Returns the checkout id and the stored order letter for one direct order from `seller`.
fn order_letter(node: &Node, title: &str, seller: &str) -> (String, SealedLetter) {
    let body = json!({"shipTo": ship_to(), "packages": [
        {"settlement": "direct", "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]}
    ]});
    let (status, value) = request(node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    let id = value["orders"][0]["letterId"].as_str().unwrap();
    let bytes = std::fs::read(node.mailbox_path().join(format!("{id}.letter"))).unwrap();
    (value["checkoutId"].as_str().unwrap().to_string(), SealedLetter::from_bytes(&bytes).unwrap())
}

#[test]
fn orders_go_to_the_records_mailbox_key_signed_by_an_unlinkable_per_order_key() {
    let (_dir, node) = testnet();
    let record = node.seller_store("Hale Paper").unwrap();
    let mailbox = node.seller_mail_secret("Hale Paper").unwrap();
    assert_eq!(*mailbox.public_key().as_bytes(), record.details.as_ref().unwrap().mailbox_key);

    let (first_checkout, first) = order_letter(&node, "Letterpress cards", "Hale Paper");
    let (_, second) = order_letter(&node, "Letterpress cards", "Hale Paper");
    let first = open_order(&first, &mailbox).unwrap();
    let second = open_order(&second, &mailbox).unwrap();
    assert_ne!(first.sender, second.sender, "two orders from one buyer do not share a sender key");
    assert_ne!(first.body.reply.key, second.body.reply.key);
    assert_ne!(first.body.reply.drop, second.body.reply.drop);
    let buyer_mail = node.buyer_mail_secret().public_key();
    assert_ne!(first.sender.0, *buyer_mail.as_bytes(), "the buyer's long-term mail key is not the sender");
    assert_eq!(first.body.reply.onion, None);
    assert_eq!(first.sender, node.order_letter_key(&first_checkout, record.id.as_bytes()).unwrap().id());

    let (reply_secret, drop) = node.order_reply_secret(&first_checkout, record.id.as_bytes()).unwrap();
    assert_eq!(drop, first.body.reply.drop);
    let vendor = SenderKey::generate();
    let reply = seal_reply(b"shipped", &first.body.reply.key, &vendor).unwrap();
    let opened = open_reply(&reply, &reply_secret).unwrap();
    assert_eq!(opened.body, b"shipped");
    assert_eq!(opened.sender, vendor.id());
}
