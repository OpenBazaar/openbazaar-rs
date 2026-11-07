//! The own store across the key tiers, the signed Monero flag, and store keys in the catalog.

use std::fs;
use std::path::Path;

use openbazaar_protocol::Store;
use serde_json::{json, Value};

use crate::harness::{checkout, open_node, request, unlock, unreachable_escrow, MockChain, TempDir};
use crate::network::NetworkMode;
use crate::own_store::PROFILE_FILE;
use crate::wallet_lock::State;
use crate::{load_catalog, Node, NodeConfig};

/// Open without unlocking.
fn open_locked(dir: &Path) -> Node {
    let config = NodeConfig {
        network: NetworkMode::Testnet4,
        confirmations: 1,
        public_server: false,
        chain: Box::new(MockChain::new()),
        escrow: unreachable_escrow(),
    };
    Node::open_with(dir, load_catalog(NetworkMode::Testnet4).unwrap(), config).unwrap()
}

/// A data dir that finished setup, closed.
fn set_up() -> TempDir {
    let dir = TempDir::new();
    drop(open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, unreachable_escrow()));
    dir
}

fn own_page(node: &Node) -> Value {
    let (status, value) = request(node, "GET", &format!("/stores/{}", node.own_store.to_hex()), "");
    assert!(status.contains("200"), "{status} {value}");
    value["store"].clone()
}

fn record(page: &Value) -> Store {
    let hex = page["record"].as_str().unwrap();
    let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
    let store = Store::decode(&bytes).unwrap();
    store.verify().unwrap();
    store
}

fn edit_profile(dir: &Path, change: impl FnOnce(&mut Value)) {
    let path = dir.join(PROFILE_FILE);
    let mut profile: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    change(&mut profile);
    fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
}

#[test]
fn a_locked_node_serves_its_last_signed_store_and_signs_changes_at_unlock() {
    let dir = set_up();
    edit_profile(&dir.0, |profile| profile["tagline"] = json!("Hand-bound notebooks"));

    let node = open_locked(&dir.0);
    assert_eq!(node.lock.state(), State::Locked);
    let page = own_page(&node);
    assert_eq!(page["tagline"], "", "the locked node serves the record it signed before");
    assert_eq!(page["unpublishedChanges"], true);
    assert_eq!(record(&page).id, node.own_store);

    unlock(&node);
    let page = own_page(&node);
    assert_eq!(page["tagline"], "Hand-bound notebooks");
    assert_eq!(page["unpublishedChanges"], false);
    record(&page);
    drop(node);

    let node = open_locked(&dir.0);
    let page = own_page(&node);
    assert_eq!((page["tagline"].as_str(), page["unpublishedChanges"].as_bool()), (Some("Hand-bound notebooks"), Some(false)));
}

#[test]
fn contact_or_location_text_in_the_profile_is_never_signed() {
    let dir = set_up();
    edit_profile(&dir.0, |profile| profile["about"] = json!("Write to shop@example.com for custom work."));
    let node = open_locked(&dir.0);
    unlock(&node);
    assert!(node.refresh_own_store().is_err());
    let page = own_page(&node);
    assert_eq!(page["about"], "", "the old record stays up");
    assert_eq!(page["unpublishedChanges"], true);
}

#[test]
fn a_watch_file_from_before_the_mailbox_key_is_upgraded_at_unlock() {
    let dir = set_up();
    let node = open_locked(&dir.0);
    let vault = node.lock.vault().clone();
    drop(node);
    let watch = dir.0.join("wallets").join("node").join("watch.obv");
    let mut value: Value = serde_json::from_slice(&vault.read_sealed(&watch).unwrap().unwrap()).unwrap();
    value.as_object_mut().unwrap().retain(|key, _| key != "storeMailboxKey" && key != "letterRoot");
    vault.write_sealed(&watch, &serde_json::to_vec(&value).unwrap()).unwrap();

    let node = open_locked(&dir.0);
    assert!(node.buyer_wallet().watch.needs_upgrade());
    let body = json!({"shipTo": crate::harness::ship_to(), "packages": [
        {"settlement": "direct", "lines": [{"listingHash": crate::harness::hash_of(&node, "Paperback novel", "Marlow Goods"), "quantity": 1}]}
    ]});
    let (status, value) = request(&node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("423") && value["code"] == "locked", "no letter key yet: {status} {value}");
    assert_eq!(own_page(&node)["unpublishedChanges"], false, "the cached record is still current");

    unlock(&node);
    assert!(!node.buyer_wallet().watch.needs_upgrade());
    let saved: Value = serde_json::from_slice(&vault.read_sealed(&watch).unwrap().unwrap()).unwrap();
    assert!(saved["storeMailboxKey"].is_string() && saved["letterRoot"].is_string());
    node.lock.lock();
    assert_eq!(checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]).len(), 1, "a locked node signs order letters");
}

#[test]
fn the_monero_choice_is_signed_into_the_store_record() {
    let dir = TempDir::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, unreachable_escrow());
    let marlow = node.seller_store("Marlow Goods").unwrap().id.to_hex();
    let accepts = |node: &Node| {
        let (_, page) = request(node, "GET", &format!("/stores/{marlow}"), "");
        let signed = record(&page["store"]).details.unwrap().accepts_xmr;
        assert_eq!(page["store"]["acceptsXmr"], signed);
        signed
    };
    assert!(!accepts(&node));

    node.lock.lock();
    let (status, value) = request(&node, "PUT", "/xmr/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    assert!(status.contains("200"), "a demo store's key is a fixture, so no unlock: {status} {value}");
    assert!(accepts(&node));
    assert!(!node.accepts_xmr("Hale Paper"));
    drop(node);

    let node = open_locked(&dir.0);
    assert!(accepts(&node), "the choice is signed again at boot");
    let (status, _) = request(&node, "PUT", "/xmr/stores", &json!({"seller": "Marlow Goods", "accept": false}).to_string());
    assert!(status.contains("200"));
    assert!(!accepts(&node));
}

#[test]
fn every_catalog_listing_names_a_store_served_at_its_key() {
    let dir = TempDir::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, unreachable_escrow());
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    let listings = catalog["listings"].as_array().unwrap();
    assert!(!listings.is_empty());
    for listing in listings {
        let key = listing["storeId"].as_str().unwrap();
        let (status, page) = request(&node, "GET", &format!("/stores/{key}"), "");
        assert!(status.contains("200"), "{key}");
        assert_eq!(page["store"]["name"], listing["seller"]);
        assert!(page["listings"].as_array().unwrap().iter().any(|own| own["hash"] == listing["hash"]));
    }
}
