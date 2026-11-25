//! Classifieds board: post, list, expire rules, and sealed contact mail.

use openbazaar_protocol::{add_days, Classified, ClassifiedCategory, ClassifiedKind, SignedClassified};
use serde_json::json;
use tormail::{seal_message, DropId, ReplyKeypair, SenderKey};

use crate::escrow_mail::hex;
use crate::harness::{open_node, request, unlock, MockChain, TempDir};
use crate::http::dispatch;
use crate::network::NetworkMode;
use crate::seller_store_tests::open_store;
use crate::{EscrowConfig, Node};

fn post_mail(node: &Node, bytes: &[u8]) -> String {
    let mut raw = format!(
        "POST /mail HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    raw.extend_from_slice(bytes);
    let response = String::from_utf8(dispatch(node, &raw)).unwrap();
    response.lines().next().unwrap().to_string()
}

fn mail_key(node: &Node) -> tormail::StoreKey {
    let catalog = node.catalog();
    let store = catalog.stores.iter().find(|store| store.id == node.own_store).unwrap();
    crate::stores::mailbox_key(store).unwrap()
}

#[test]
fn a_classified_is_posted_listed_and_survives_restart() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let id = {
        let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
        let _ = open_store(&node);
        unlock(&node);
        let body = json!({
            "kind": "wtb",
            "category": "goods",
            "title": "Looking for a spare bike chain",
            "body": "Prefer an 11-speed chain in good shape. Local pickup.",
            "region": "Oregon",
            "priceHint": 2500,
            "currency": "USD",
            "lifeDays": 7,
        });
        let (status, post) = request(&node, "POST", "/classifieds", &body.to_string());
        assert!(status.contains("200"), "{status} {post}");
        assert_eq!(post["kind"], "wtb");
        assert_eq!(post["category"], "goods");
        assert_eq!(post["title"], "Looking for a spare bike chain");
        assert_eq!(post["lifeDays"], 7);
        assert_eq!(post["expired"], false);
        let id = post["id"].as_str().unwrap().to_string();

        let (status, listed) = request(&node, "GET", "/classifieds", "");
        assert!(status.contains("200"), "{status} {listed}");
        assert_eq!(listed["classifieds"].as_array().unwrap().len(), 1);
        assert_eq!(listed["classifieds"][0]["id"], id);

        let (status, one) = request(&node, "GET", &format!("/classifieds/{id}"), "");
        assert!(status.contains("200"), "{status} {one}");
        assert_eq!(one["body"], "Prefer an 11-speed chain in good shape. Local pickup.");

        let (status, filtered) = request(&node, "GET", "/classifieds?kind=wts", "");
        assert!(status.contains("200"), "{status} {filtered}");
        assert!(filtered["classifieds"].as_array().unwrap().is_empty());

        let leaky = json!({
            "kind": "wts",
            "category": "services",
            "title": "Call me at 503 555 0142",
            "body": "Handyman available weekends.",
            "lifeDays": 7,
        });
        let (status, err) = request(&node, "POST", "/classifieds", &leaky.to_string());
        assert!(status.contains("400"), "{status} {err}");
        id
    };

    let node = Node::open(&dir.0, crate::load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    let (status, listed) = request(&node, "GET", "/classifieds", "");
    assert!(status.contains("200"), "{status} {listed}");
    assert_eq!(listed["classifieds"].as_array().unwrap().len(), 1);
    assert_eq!(listed["classifieds"][0]["id"], id);

    unlock(&node);
    let (status, _) = request(&node, "DELETE", &format!("/classifieds/{id}"), "");
    assert!(status.contains("200"), "{status}");
    assert!(request(&node, "GET", "/classifieds", "").1["classifieds"].as_array().unwrap().is_empty());
}

#[test]
fn contact_mail_opens_a_thread_on_the_poster() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
    let _ = open_store(&node);
    unlock(&node);
    let (status, post) = request(
        &node,
        "POST",
        "/classifieds",
        &json!({
            "kind": "wts",
            "category": "goods",
            "title": "Stoneware mug for sale",
            "body": "One unused mug from a small kiln run.",
            "lifeDays": 14,
        })
        .to_string(),
    );
    assert!(status.contains("200"), "{status} {post}");
    let classified_id = post["id"].as_str().unwrap().to_string();

    let sender = SenderKey::generate();
    let reply = ReplyKeypair::generate();
    let drop = DropId::generate();
    let message_id = "a".repeat(32);
    let body = json!({
        "v": 1,
        "kind": "contact",
        "classified": classified_id,
        "id": message_id,
        "text": "Still available?",
        "replyUrl": "http://127.0.0.1:9",
        "replyKey": hex(reply.public_key().as_bytes()),
        "drop": hex(&drop.0),
        "title": "Stoneware mug for sale",
    });
    let letter = seal_message(&serde_json::to_vec(&body).unwrap(), &mail_key(&node), &sender).unwrap().to_bytes();
    let status = post_mail(&node, &letter);
    assert!(status.contains("200"), "{status}");

    let (status, threads) = request(&node, "GET", "/contact", "");
    assert!(status.contains("200"), "{status} {threads}");
    let list = threads["threads"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["classifiedId"], classified_id);
    assert_eq!(list[0]["asPoster"], true);
    assert_eq!(list[0]["messages"][0]["text"], "Still available?");
    assert_eq!(list[0]["messages"][0]["from"], "them");
    let thread_id = list[0]["id"].as_str().unwrap().to_string();

    let (status, replied) = request(&node, "POST", &format!("/contact/{thread_id}/message"), r#"{"text":"Yes, still here."}"#);
    assert!(status.contains("200"), "{status} {replied}");
    assert_eq!(replied["messages"].as_array().unwrap().len(), 2);
    assert_eq!(replied["messages"][1]["from"], "me");
    assert_eq!(replied["messages"][1]["text"], "Yes, still here.");

    // Redelivery of the same letter is one message.
    let again = post_mail(&node, &letter);
    assert!(again.contains("200"), "{again}");
    assert_eq!(request(&node, "GET", &format!("/contact/{thread_id}"), "").1["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn expired_posts_drop_from_the_live_board() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
    let _ = open_store(&node);
    unlock(&node);
    let secret = node.own_signing_key().unwrap();
    let poster = openbazaar_protocol::xonly(&secret);
    let mailbox = {
        let wallet = node.buyer_wallet();
        let key = *wallet.watch.store_mailbox_key().unwrap();
        *tormail::StoreSecret::from_bytes(key).unwrap().public_key().as_bytes()
    };
    let created_on = 20250101;
    let expires_on = add_days(created_on, 7).unwrap();
    let signed = SignedClassified::sign(
        Classified {
            id: [9u8; 16],
            kind: ClassifiedKind::Wtb,
            category: ClassifiedCategory::Other,
            title: "Old post that already expired".into(),
            body: "This should not show on the live board.".into(),
            region: String::new(),
            price_hint: 0,
            currency: String::new(),
            created_on,
            expires_on,
            poster,
            mailbox_key: mailbox,
            onion: String::new(),
        },
        &secret,
    )
    .unwrap();
    assert!(node.accept_classified(&signed.encode().unwrap()).unwrap());
    let (status, listed) = request(&node, "GET", "/classifieds", "");
    assert!(status.contains("200"), "{status} {listed}");
    assert!(listed["classifieds"].as_array().unwrap().is_empty(), "expired posts stay off the live board");
    assert_eq!(node.classifieds().posts.len(), 1, "the sealed pool still keeps the expired record");
}
