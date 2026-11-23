//! Sending Zcash to an address the user names, on the mock chain.

use std::path::Path;
use std::sync::Arc;

use openbazaar_zec::mock::MockChain as ZecChainMock;
use openbazaar_zec::CONFIRMATIONS;
use serde_json::json;

use crate::harness::{open_node, request, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::network::NetworkMode;
use crate::Node;

fn open(dir: &Path, btc: &MockChain, zec: &Arc<ZecChainMock>) -> Node {
    let node = open_node(dir, NetworkMode::Testnet4, btc, 1, unreachable_escrow());
    node.set_zec_chain(zec.clone()).unwrap();
    node
}

fn receive(node: &Node) -> String {
    request(node, "GET", "/zec/wallet", "").1["receiveAddress"].as_str().unwrap().to_string()
}

fn confirmed(node: &Node) -> String {
    request(node, "GET", "/zec/wallet", "").1["balance"]["confirmedZatoshi"].as_str().unwrap().to_string()
}

#[test]
fn one_node_sends_testnet_zcash_to_another_once() {
    let dirs = [TempDir::new(), TempDir::new()];
    let btc = MockChain::new();
    let zec = Arc::new(ZecChainMock::new(NetworkMode::Testnet4.zcash()));
    let alice = open(&dirs[0].0, &btc, &zec);
    let bob = open(&dirs[1].0, &btc, &zec);
    alice.zec_sync_once().unwrap();
    bob.zec_sync_once().unwrap();
    let to = receive(&bob);
    zec.pay(&receive(&alice), 100_000_000);
    zec.mine(CONFIRMATIONS);
    alice.zec_sync_once().unwrap();
    assert_eq!(confirmed(&alice), "100000000");

    let (status, terms) = request(
        &alice,
        "POST",
        "/zec/wallet/send/quote",
        &json!({ "address": to, "amountZatoshi": "10000000" }).to_string(),
    );
    assert!(status.contains("200"), "{status} {terms}");
    assert_eq!(terms["network"], "testnet");
    assert_eq!(terms["address"], to.as_str());
    assert_eq!(terms["amountZec"], "0.1");
    let fee: u64 = terms["feeZatoshi"].as_str().unwrap().parse().unwrap();
    assert!(fee > 0);
    assert_eq!(terms["totalZatoshi"], (10_000_000 + fee).to_string());

    let bare = json!({ "quote": terms["quote"], "passphrase": PASSPHRASE }).to_string();
    let (status, value) = request(&alice, "POST", "/zec/wallet/send", &bare);
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("confirm"), "{status} {value}");
    let wrong = json!({ "quote": terms["quote"], "passphrase": "not the passphrase", "confirm": true }).to_string();
    let (status, value) = request(&alice, "POST", "/zec/wallet/send", &wrong);
    assert!(status.contains("400") && value["code"] == "wrong_passphrase", "{status} {value}");

    let body = json!({ "quote": terms["quote"], "passphrase": PASSPHRASE, "confirm": true }).to_string();
    let (status, sent) = request(&alice, "POST", "/zec/wallet/send", &body);
    assert!(status.contains("200"), "{status} {sent}");
    assert_eq!(sent["txid"].as_str().unwrap().len(), 64);
    let (status, _) = request(&alice, "POST", "/zec/wallet/send", &body);
    assert!(status.contains("409"), "a quote sends once: {status}");

    zec.mine(CONFIRMATIONS);
    bob.zec_sync_once().unwrap();
    assert_eq!(confirmed(&bob), "10000000");
}

#[test]
fn a_mainnet_node_does_not_send_zcash_from_the_wallet() {
    let dir = TempDir::new();
    let node = open_node(&dir.0, NetworkMode::Mainnet, &MockChain::new(), 1, unreachable_escrow());
    let (status, value) = request(
        &node,
        "POST",
        "/zec/wallet/send/quote",
        &json!({ "address": "u1example", "amountZatoshi": "1" }).to_string(),
    );
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("testnet only"), "{status} {value}");
    let _keep = value;
}
