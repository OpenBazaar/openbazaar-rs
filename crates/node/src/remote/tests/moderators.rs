//! A paired device runs the moderator desk and the store's moderator list.

use serde_json::json;

use super::*;
use crate::harness::{open_node, MockChain};
use crate::EscrowConfig;

#[test]
fn a_device_session_edits_the_moderator_profile_and_the_accepted_moderators() {
    let dir = TempDir::new();
    let escrow = EscrowConfig { mailbox_url: Some("http://127.0.0.1:1".into()), moderates: true, ..EscrowConfig::default() };
    let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, escrow);
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    let device = first_device(&node);

    for (method, path) in [("GET", "/moderator/profile"), ("PUT", "/moderator/profile"), ("GET", "/moderators"), ("POST", "/moderators")] {
        let (status, value) = send(&node, Entry::Admin, &unsigned(method, path, "", "{}"));
        assert!(status.contains("401"), "{method} {path} without a session: {status} {value}");
    }

    let (status, saved) = device.call(&node, "PUT", "/moderator/profile", &json!({ "name": "Ines Varga", "feeSats": 1500 }).to_string());
    assert!(status.contains("200"), "{status} {saved}");
    let (status, profile) = device.call(&node, "GET", "/moderator/profile", "");
    assert!(status.contains("200"), "{status} {profile}");
    assert_eq!((&profile["name"], &profile["feeSats"], &profile["published"]), (&json!("Ines Varga"), &json!(1500), &json!(true)));

    let (status, list) = device.call(&node, "GET", "/moderators", "");
    assert!(status.contains("200"), "{status} {list}");
    let builtin = list["moderators"][0]["id"].as_str().unwrap().to_string();
    assert_eq!((&list["moderators"][0]["builtIn"], &list["moderators"][0]["accepted"]), (&json!(true), &json!(true)), "{list}");
    let store = crate::escrow_mail::hex(node.seller_store("Marlow Goods").unwrap().id.as_bytes());
    let (status, offered) = device.call(&node, "GET", &format!("/stores/{store}/moderators"), "");
    assert!(status.contains("200"), "{status} {offered}");
    assert_eq!(offered["moderators"][0]["id"], json!(builtin), "{offered}");

    let (status, removed) = device.call(&node, "DELETE", &format!("/moderators/{builtin}"), "");
    assert!(status.contains("200"), "{status} {removed}");
    assert_eq!(removed["moderators"][0]["accepted"], false, "{removed}");
    let (status, added) = device.call(&node, "POST", "/moderators", &json!({ "id": builtin }).to_string());
    assert!(status.contains("200"), "{status} {added}");
    assert_eq!(added["moderators"][0]["accepted"], true, "{added}");
}
