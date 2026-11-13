//! What a locked node still does (the receive tier), and the migration of a
//! data dir from before the lock.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::harness::{
    address_any, checkout, order, request, unlock, unreachable_escrow, Market, MockChain, TempDir, PASSPHRASE,
};
use crate::network::NetworkMode;
use crate::wallet_keys::{SEED_FILE, SPEND_FILE};
use crate::wallet_lock::State;
use crate::{load_catalog, Node, NodeConfig, AWAITING_FUNDING, AWAITING_TERMS};

/// Open without unlocking (the harness `open_node` unlocks).
fn open(dir: &Path) -> Node {
    let config = NodeConfig {
        network: NetworkMode::Testnet4,
        confirmations: 1,
        public_server: false,
        chain: Box::new(MockChain::new()),
        escrow: unreachable_escrow(),
    };
    Node::open_with(dir, load_catalog(NetworkMode::Testnet4).unwrap(), config).unwrap()
}

fn wallet_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir.join("wallets"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    dirs.sort();
    dirs
}

fn receive_address(node: &Node) -> String {
    let (status, wallet) = request(node, "GET", "/wallet", "");
    assert!(status.contains("200"), "{status} {wallet}");
    wallet["receiveAddress"].as_str().unwrap().to_string()
}

/// A data dir from before the lock: phrases in `seed.obv` under the vault
/// key, no `watch.obv`. The wallet state was always public descriptors.
fn legacy_dir() -> (TempDir, Vec<(PathBuf, Vec<u8>)>, String) {
    let dir = TempDir::new();
    let node = open(&dir.0);
    let address = receive_address(&node);
    let vault = node.lock.vault().clone();
    drop(node);
    let mut phrases = Vec::new();
    for wallet in wallet_dirs(&dir.0) {
        fs::remove_file(wallet.join("watch.obv")).unwrap();
        phrases.push((wallet.clone(), vault.read_sealed(&wallet.join(SEED_FILE)).unwrap().unwrap()));
    }
    assert!(phrases.len() > 1, "the node wallet and the store wallets");
    (dir, phrases, address)
}

#[test]
fn an_old_data_dir_refuses_spends_until_a_passphrase_moves_the_seeds() {
    let (dir, phrases, address) = legacy_dir();
    let node = open(&dir.0);
    assert_eq!(node.lock.state(), State::Setup);
    assert_eq!(receive_address(&node), address, "the same wallet, opened watch-only");
    node.watch_once().unwrap();
    let (status, value) = request(&node, "POST", "/wallet/phrase", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("423") && value["code"] == "setup_required", "{status} {value}");
    assert_eq!(request(&node, "GET", "/lock", "").1["state"], "setup", "the client prompts for a passphrase");

    // A setup that died before writing wallet.lock left a spend.obv behind.
    fs::write(phrases[0].0.join(SPEND_FILE), b"half written").unwrap();
    drop(node);
    let node = open(&dir.0);
    assert!(!phrases[0].0.join(SPEND_FILE).exists(), "an uncommitted spend file is dropped");

    let (status, setup) = request(&node, "POST", "/setup", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("200"), "{setup}");
    assert_eq!(setup["phrase"].as_str().unwrap().as_bytes(), &phrases[0].1[..], "the backup is the old phrase");
    for (wallet, _) in &phrases {
        assert!(!wallet.join(SEED_FILE).exists(), "{}", wallet.display());
        assert!(wallet.join(SPEND_FILE).exists(), "{}", wallet.display());
    }
    assert_eq!(receive_address(&node), address);
    drop(node);

    let node = open(&dir.0);
    assert_eq!(node.lock.state(), State::Locked);
    unlock(&node);
    let key = node.lock.spend_key().unwrap();
    for (wallet, phrase) in &phrases {
        let read = crate::wallet_keys::read_phrase(wallet, &key).unwrap();
        assert_eq!(read.as_bytes(), &phrase[..], "{}", wallet.display());
    }
}

#[test]
fn a_locked_node_still_receives() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = crate::harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, unreachable_escrow());
    node.lock.lock();
    let first = receive_address(&node);
    let (status, fresh) = request(&node, "POST", "/wallet/address", "");
    assert!(status.contains("200"), "{status} {fresh}");
    assert_ne!(fresh["address"], json!(first), "new addresses come from the xpub");
    chain.fund_wallet(&address_any(&first), 50_000);
    node.watch_once().unwrap();
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    assert_eq!(wallet["balance"]["confirmedSats"], 50_000, "the watcher sees coins while locked: {wallet}");
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    assert!(placed[0]["payment"]["address"].is_string(), "a direct order gets its address: {}", placed[0]);
    assert_eq!(node.lock.state(), State::Locked, "none of this unlocked or touched the wallet");
}

#[test]
fn escrow_letters_that_sign_wait_for_an_unlock() {
    let market = Market::new();
    let (buyer, vendor) = (&market.buyer.node, &market.vendor.node);
    vendor.lock.lock();
    let placed = checkout(buyer, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    assert_eq!(market.deliver(), 0, "the locked vendor keeps the request");
    assert_eq!(vendor.escrow.inbox.list().unwrap().len(), 1);
    assert!(request(vendor, "GET", "/wallet", "").0.contains("200"));

    unlock(vendor);
    buyer.lock.lock();
    assert_eq!(market.deliver(), 1, "the vendor answers; the locked buyer keeps the offer");
    assert_eq!(order(buyer, &id)["status"], AWAITING_TERMS);
    assert_eq!(buyer.escrow.inbox.list().unwrap().len(), 1);

    unlock(buyer);
    assert_eq!(market.deliver(), 1);
    assert_eq!(order(buyer, &id)["status"], AWAITING_FUNDING);
}

#[test]
fn a_dropped_escrow_request_is_sent_again_while_waiting_for_terms() {
    let market = Market::new();
    let (buyer, vendor) = (&market.buyer.node, &market.vendor.node);
    let placed = checkout(buyer, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    let waiting = vendor.escrow.inbox.list().unwrap();
    assert_eq!(waiting.len(), 1, "checkout posted the request");
    let _ = vendor.escrow.inbox.take(&waiting[0]).unwrap();
    assert_eq!(market.deliver(), 0, "the vendor dropped the accepted letter");
    assert_eq!(order(buyer, &id)["status"], AWAITING_TERMS);
    assert_eq!(buyer.retry_escrow_requests(false), 0, "the first send is still within backoff");
    assert_eq!(buyer.retry_escrow_requests(true), 1);
    assert_eq!(market.deliver(), 2, "the resent request is answered and the offer is countersigned");
    assert_eq!(order(buyer, &id)["status"], AWAITING_FUNDING);
}
