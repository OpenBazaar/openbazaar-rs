//! Sending Monero to an address the user names, on a mocked daemon.

use std::sync::Arc;

use openbazaar_xmr::mock::MockChain as XmrChainMock;
use openbazaar_xmr::{address_for, SpendKey, XmrNetwork};
use serde_json::{json, Value};

use crate::harness::{open_node, request, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::network::NetworkMode;
use crate::Node;

const XMR: u64 = 1_000_000_000_000;
const NET: XmrNetwork = XmrNetwork::Stagenet;

fn open(dir: &TempDir, btc: &MockChain, xmr: &Arc<XmrChainMock>) -> Node {
    let node = open_node(&dir.0, NetworkMode::Testnet4, btc, 1, unreachable_escrow());
    node.set_xmr_chain(xmr.clone()).unwrap();
    node.xmr_sync_once().unwrap();
    node
}

fn primary(node: &Node) -> String {
    request(node, "GET", "/xmr/wallet", "").1["primaryAddress"]
        .as_str()
        .unwrap()
        .to_string()
}

fn unlocked(node: &Node) -> u64 {
    let balance = request(node, "GET", "/xmr/wallet", "").1["balance"].clone();
    balance["unlockedPiconero"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Alice's node with 5 spendable XMR, and Bob's on the same chain.
fn two_nodes() -> (Node, Node, Arc<XmrChainMock>, [TempDir; 2], MockChain) {
    let dirs = [TempDir::new(), TempDir::new()];
    let btc = MockChain::new();
    let xmr = Arc::new(XmrChainMock::new(NET));
    let alice = open(&dirs[0], &btc, &xmr);
    let bob = open(&dirs[1], &btc, &xmr);
    xmr.mine_to(&address_for(&primary(&alice), NET).unwrap(), 5 * XMR);
    xmr.mine(10);
    alice.xmr_sync_once().unwrap();
    assert_eq!(unlocked(&alice), 5 * XMR);
    (alice, bob, xmr, dirs, btc)
}

fn quote(node: &Node, body: Value) -> (String, Value) {
    request(node, "POST", "/xmr/wallet/send/quote", &body.to_string())
}

fn send(node: &Node, quote: &Value, passphrase: &str) -> (String, Value) {
    let body = json!({ "quote": quote["quote"], "passphrase": passphrase, "confirm": true });
    request(node, "POST", "/xmr/wallet/send", &body.to_string())
}

#[test]
fn one_node_sends_monero_to_another_once() {
    let (alice, bob, xmr, _dirs, btc) = two_nodes();
    let to = primary(&bob);
    let (status, terms) = quote(
        &alice,
        json!({ "address": to, "amountPiconero": XMR.to_string() }),
    );
    assert!(status.contains("200"), "{status} {terms}");
    assert_eq!(terms["network"], "stagenet");
    assert_eq!(terms["address"], to.as_str());
    assert_eq!(terms["amountXmr"], "1");
    let fee: u64 = terms["feePiconero"].as_str().unwrap().parse().unwrap();
    assert!(fee > 0);
    assert_eq!(terms["totalPiconero"], (XMR + fee).to_string());

    let body = json!({ "quote": terms["quote"], "passphrase": PASSPHRASE }).to_string();
    let (status, value) = request(&alice, "POST", "/xmr/wallet/send", &body);
    assert!(
        status.contains("400") && value["error"].as_str().unwrap().contains("confirm"),
        "{status} {value}"
    );
    let (status, value) = send(&alice, &terms, "not the passphrase");
    assert!(
        status.contains("400") && value["code"] == "wrong_passphrase",
        "{status} {value}"
    );
    assert!(xmr.published().is_empty());

    let (status, sent) = send(&alice, &terms, PASSPHRASE);
    assert!(
        status.contains("200"),
        "a wrong passphrase left the quote open: {status} {sent}"
    );
    assert_eq!(sent["txid"].as_str().unwrap().len(), 64);
    assert_eq!(xmr.published().len(), 1);
    let (status, _) = send(&alice, &terms, PASSPHRASE);
    assert!(status.contains("409"), "a quote sends once: {status}");
    assert_eq!(xmr.published().len(), 1);
    assert!(btc.broadcasts().is_empty(), "no bitcoin moved");

    xmr.mine(10);
    alice.xmr_sync_once().unwrap();
    bob.xmr_sync_once().unwrap();
    assert_eq!(unlocked(&bob), XMR);
    let fee_paid: u64 = sent["feePiconero"].as_str().unwrap().parse().unwrap();
    assert!(
        unlocked(&alice) <= 4 * XMR - fee_paid,
        "change is spendable after its confirmations"
    );
}

#[test]
fn bad_addresses_and_amounts_are_refused_before_anything_is_signed() {
    let (alice, bob, xmr, _dirs, _btc) = two_nodes();
    let to = primary(&bob);
    let mainnet = SpendKey::from_seed_secret(&[7; 32])
        .view_keys()
        .primary_address(XmrNetwork::Mainnet)
        .to_string();
    for (body, says) in [
        (
            json!({ "address": mainnet, "amountPiconero": "1" }),
            "stagenet",
        ),
        (
            json!({ "address": "4notanaddress", "amountPiconero": "1" }),
            "stagenet",
        ),
        (json!({ "address": "", "amountPiconero": "1" }), "stagenet"),
        (
            json!({ "address": to, "amountPiconero": "0" }),
            "above zero",
        ),
        (json!({ "address": to, "amountPiconero": 1 }), "as a string"),
        (
            json!({ "address": to, "amountPiconero": "1.5" }),
            "whole number",
        ),
        (
            json!({ "address": to, "amountPiconero": "-1" }),
            "whole number",
        ),
        (
            json!({ "address": to, "amountPiconero": "99999999999999999999" }),
            "whole number",
        ),
        (
            json!({ "address": to, "amountPiconero": (6 * XMR).to_string() }),
            "This needs",
        ),
        (
            json!({ "address": to, "amountPiconero": "1", "max": true }),
            "not part",
        ),
    ] {
        let (status, value) = quote(&alice, body.clone());
        assert!(status.contains("400"), "{body}: {status} {value}");
        assert!(
            value["error"].as_str().unwrap().contains(says),
            "{body}: {value}"
        );
    }
    assert!(xmr.published().is_empty());
}

#[test]
fn an_unreachable_daemon_uses_the_quote_up_and_sends_nothing() {
    let (alice, bob, xmr, _dirs, _btc) = two_nodes();
    let (status, terms) = quote(
        &alice,
        json!({ "address": primary(&bob), "amountPiconero": XMR.to_string() }),
    );
    assert!(status.contains("200"), "{status} {terms}");
    xmr.set_offline(true);
    let (status, value) = send(&alice, &terms, PASSPHRASE);
    assert!(status.contains("502"), "{status} {value}");
    xmr.set_offline(false);
    let (status, _) = send(&alice, &terms, PASSPHRASE);
    assert!(status.contains("409"), "{status}");
    assert!(xmr.published().is_empty());
    assert_eq!(unlocked(&alice), 5 * XMR);
}

#[test]
fn a_locked_wallet_sends_nothing_and_a_lock_drops_open_quotes() {
    let (alice, bob, xmr, _dirs, _btc) = two_nodes();
    let body = json!({ "address": primary(&bob), "amountPiconero": XMR.to_string() });
    let (_, terms) = quote(&alice, body.clone());
    alice.lock.lock();
    let (status, value) = quote(&alice, body);
    assert!(
        status.contains("423") && value["code"] == "locked",
        "{status} {value}"
    );
    crate::harness::unlock(&alice);
    let (status, _) = send(&alice, &terms, PASSPHRASE);
    assert!(
        status.contains("409"),
        "the lock dropped the quote: {status}"
    );
    assert!(xmr.published().is_empty());
}
