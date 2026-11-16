//! Paying a pasted invoice and withdrawing on-chain funds, on mocked Lightning nodes.

use std::sync::Arc;

use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::mock::MockRail;
use openbazaar_lightning::{Incoming, LightningRail};
use serde_json::{json, Value};

use crate::harness::{request, unlock, MockChain, TempDir, PASSPHRASE};
use crate::Node;

use super::tests::open;

const SIGNET: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
const MAINNET: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";

/// A node whose Lightning node has 50,000 sats to spend and 100,000 on chain.
fn setup() -> (Node, Arc<MockRail>, TempDir, MockChain) {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let rail = Arc::new(MockRail::new(Network::Signet));
    let node = open(&dir.0, &btc, &rail);
    rail.set_lightning_sats(50_000);
    rail.set_onchain_sats(100_000);
    (node, rail, dir, btc)
}

fn post(node: &Node, path: &str, body: Value) -> (String, Value) {
    request(node, "POST", path, &body.to_string())
}

fn confirm(quote: &Value, passphrase: &str) -> Value {
    json!({ "quote": quote["quote"], "passphrase": passphrase, "confirm": true })
}

#[test]
fn pays_an_invoice_from_another_wallet_once() {
    let (node, rail, _dir, _btc) = setup();
    let seller = rail.linked();
    let invoice = seller.receive(20_000_000, "coffee", 600).unwrap();
    let (status, quote) = post(
        &node,
        "/ln/send/quote",
        json!({ "invoice": format!("lightning:{}", invoice.bolt11) }),
    );
    assert!(status.contains("200"), "{status} {quote}");
    assert_eq!(quote["amountSats"], 20_000);
    assert_eq!(quote["feeCapSats"], 200);
    assert_eq!(quote["description"], "coffee");
    assert_eq!(quote["paymentHash"], invoice.payment_hash.as_str());
    assert_eq!(quote["network"], "signet");

    let (status, value) = post(
        &node,
        "/ln/send",
        json!({ "quote": quote["quote"], "passphrase": PASSPHRASE }),
    );
    assert!(status.contains("400"), "{status} {value}");
    let (status, value) = post(&node, "/ln/send", confirm(&quote, "wrong"));
    assert!(
        status.contains("400") && value["code"] == "wrong_passphrase",
        "{status} {value}"
    );
    assert_eq!(seller.incoming(&invoice.payment_hash), Incoming::Pending);

    let (status, sent) = post(&node, "/ln/send", confirm(&quote, PASSPHRASE));
    assert!(status.contains("200"), "{status} {sent}");
    assert_eq!(sent["status"], "paid");
    assert!(matches!(
        seller.incoming(&invoice.payment_hash),
        Incoming::Paid {
            received_msat: 20_000_000,
            ..
        }
    ));
    assert_eq!(rail.balances().lightning_sats, 30_000);
    let (status, _) = post(&node, "/ln/send", confirm(&quote, PASSPHRASE));
    assert!(status.contains("409"), "a quote pays once: {status}");
    assert_eq!(rail.balances().lightning_sats, 30_000);
}

#[test]
fn unusable_invoices_are_refused_before_anything_is_paid() {
    let (node, rail, _dir, _btc) = setup();
    let seller = rail.linked();
    let mainnet = MockRail::new(Network::Bitcoin);
    let cases = [
        (
            rail.receive(1_000_000, "own", 600).unwrap().bolt11,
            "This node issued",
        ),
        (
            mainnet.receive(1_000_000, "real", 600).unwrap().bolt11,
            "not signet",
        ),
        (
            seller.receive(1_000_000, "late", 0).unwrap().bolt11,
            "expired",
        ),
        (
            seller.receive(60_000_000, "big", 600).unwrap().bolt11,
            "Not enough funds",
        ),
        ("lntbs1garbage".to_string(), "not a BOLT 11"),
        (String::new(), "not a BOLT 11"),
    ];
    for (invoice, says) in cases {
        let (status, value) = post(&node, "/ln/send/quote", json!({ "invoice": invoice }));
        assert!(
            status.contains("400") && value["error"].as_str().unwrap().contains(says),
            "{says}: {status} {value}"
        );
    }
    let (status, _) = post(
        &node,
        "/ln/send/quote",
        json!({ "invoice": "x", "amountSats": 5 }),
    );
    assert!(status.contains("400"));
    assert_eq!(rail.balances().lightning_sats, 50_000);
}

#[test]
fn withdraws_an_amount_or_everything_to_an_address_on_the_lightning_network() {
    let (node, rail, _dir, _btc) = setup();
    for (body, says) in [
        (
            json!({ "address": MAINNET, "amountSats": 25_000 }),
            "not on signet",
        ),
        (
            json!({ "address": "nope", "amountSats": 25_000 }),
            "not a bitcoin address",
        ),
        (json!({ "address": SIGNET, "amountSats": 100 }), "dust"),
        (
            json!({ "address": SIGNET, "amountSats": 100_000 }),
            "Not enough funds",
        ),
        (json!({ "address": SIGNET, "amountSats": 0 }), "whole sats"),
        (
            json!({ "address": SIGNET, "amountSats": "5000" }),
            "whole sats",
        ),
        (json!({ "address": SIGNET }), "whole sats"),
        (
            json!({ "address": SIGNET, "amountSats": 5_000, "max": true }),
            "not both",
        ),
    ] {
        let (status, value) = post(&node, "/ln/withdraw/quote", body.clone());
        assert!(
            status.contains("400") && value["error"].as_str().unwrap().contains(says),
            "{body}: {status} {value}"
        );
    }

    let (status, quote) = post(
        &node,
        "/ln/withdraw/quote",
        json!({ "address": SIGNET, "amountSats": 25_000 }),
    );
    assert!(status.contains("200"), "{status} {quote}");
    assert_eq!(quote["amountSats"], 25_000);
    assert_eq!(quote["max"], false);
    let (status, value) = post(&node, "/ln/send", confirm(&quote, PASSPHRASE));
    assert!(
        status.contains("409"),
        "a withdrawal quote does not pay an invoice: {status} {value}"
    );
    assert!(rail.onchain_sends().is_empty());

    let (_, quote) = post(
        &node,
        "/ln/withdraw/quote",
        json!({ "address": SIGNET, "amountSats": 25_000 }),
    );
    let (status, sent) = post(&node, "/ln/withdraw", confirm(&quote, PASSPHRASE));
    assert!(
        status.contains("200") && sent["txid"].is_string(),
        "{status} {sent}"
    );
    let (_, quote) = post(
        &node,
        "/ln/withdraw/quote",
        json!({ "address": SIGNET, "max": true }),
    );
    assert_eq!(quote["amountSats"], 75_000);
    assert_eq!(quote["max"], true);
    let (status, _) = post(&node, "/ln/withdraw", confirm(&quote, PASSPHRASE));
    assert!(status.contains("200"));
    assert_eq!(
        rail.onchain_sends(),
        vec![
            (SIGNET.to_string(), Some(25_000)),
            (SIGNET.to_string(), None)
        ]
    );
    let (status, _) = post(&node, "/ln/withdraw", confirm(&quote, PASSPHRASE));
    assert!(status.contains("409"));
    let (status, value) = post(
        &node,
        "/ln/withdraw/quote",
        json!({ "address": SIGNET, "max": true }),
    );
    assert!(
        status.contains("400")
            && value["error"]
                .as_str()
                .unwrap()
                .contains("Not enough funds"),
        "{status} {value}"
    );
}

#[test]
fn a_locked_wallet_sends_nothing_and_a_lock_drops_open_quotes() {
    let (node, rail, _dir, _btc) = setup();
    let invoice = rail.linked().receive(1_000_000, "tea", 600).unwrap().bolt11;
    let (_, quote) = post(&node, "/ln/send/quote", json!({ "invoice": invoice }));
    node.lock.lock();
    let (status, value) = post(
        &node,
        "/ln/withdraw/quote",
        json!({ "address": SIGNET, "max": true }),
    );
    assert!(status.contains("423"), "{status} {value}");
    let (status, _) = post(&node, "/ln/send", confirm(&quote, PASSPHRASE));
    assert!(status.contains("423"));
    unlock(&node);
    let (status, _) = post(&node, "/ln/send", confirm(&quote, PASSPHRASE));
    assert!(
        status.contains("409"),
        "the lock dropped the quote: {status}"
    );
    assert_eq!(rail.balances().lightning_sats, 50_000);
}
