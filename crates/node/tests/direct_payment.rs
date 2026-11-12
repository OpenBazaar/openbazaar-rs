//! One real `ob-node` process takes a direct bitcoin order for a demo store
//! from checkout to Paid.
//!
//! The in-process tests (`payment_tests.rs`) cover the same order on a mock
//! chain and step the watcher by hand. Here the binary's own watcher asks a
//! stand-in Esplora server on loopback, through its real chain client, and
//! someone outside the node pays the order's address in two parts. The node's
//! own wallet sends nothing: the stand-in serves no wallet scan.

#[allow(dead_code)]
mod common;

use std::time::Duration;

use common::esplora::Esplora;
use common::{wait_for, RealNode};
use serde_json::{json, Value};

const PASSPHRASE: &str = "correct horse battery";

#[test]
fn a_real_node_takes_a_part_payment_then_the_rest_and_marks_the_order_paid() {
    let chain = Esplora::start();
    let limit = Duration::from_secs(30);
    let node = RealNode::start("buyer", &chain.url, &["--confirmations", "2"], limit);
    // The answer carries the new wallet's phrase, so it is not printed.
    let (status, _) = node.request("POST", "/setup", &json!({ "passphrase": PASSPHRASE }).to_string());
    assert_eq!(status, 200, "POST /setup");
    let placed = checkout(&node);
    assert_eq!(placed["status"], "Awaiting payment", "{placed}");
    let address = placed["payment"]["address"].as_str().unwrap();
    let total = placed["totalSats"].as_u64().unwrap();
    let path = format!("/orders/{}", placed["orderId"].as_str().unwrap());
    let order_when = |step: &str, ready: &dyn Fn(&Value) -> bool| {
        wait_for(step, limit, || {
            let (_, order) = node.request("GET", &path, "");
            if ready(&order) { Ok(order) } else { Err(order.to_string()) }
        })
    };

    chain.pay(address, total - 1_000);
    let part = order_when("the part payment to be seen", &|order| order["receivedSats"] == total - 1_000);
    assert_eq!(part["status"], "Awaiting payment", "{part}");
    chain.mine();
    let part = order_when("the part payment to confirm", &|order| order["confirmations"] == 1);
    assert_eq!((&part["status"], &part["receivedSats"]), (&json!("Awaiting payment"), &json!(total - 1_000)), "{part}");

    chain.pay(address, 1_000);
    let seen = order_when("the rest to be seen", &|order| order["receivedSats"] == total);
    assert_eq!((&seen["status"], &seen["confirmations"]), (&json!("Payment seen"), &json!(0)), "{seen}");
    chain.mine();
    let seen = order_when("the rest to confirm once", &|order| order["confirmations"] == 1);
    assert_eq!(seen["status"], "Payment seen", "two confirmations are required: {seen}");
    chain.mine();
    let paid = order_when("the order to be paid", &|order| order["status"] == "Paid");
    assert_eq!((&paid["receivedSats"], &paid["confirmations"]), (&json!(total), &json!(2)), "{paid}");
    assert_eq!(paid["txids"].as_array().unwrap().len(), 2, "{paid}");
    assert!(!node.data_dir().join("tor-client").exists(), "the node opened the Tor client that dials the introductions");
}

/// Check out one demo-store listing for direct payment. Returns the order.
fn checkout(node: &RealNode) -> Value {
    let (status, catalog) = node.request("GET", "/catalog", "");
    assert_eq!(status, 200, "{catalog}");
    let listings = catalog["listings"].as_array().unwrap();
    let listing = listings.iter().find(|item| item["title"] == "Paperback novel" && item["seller"] == "Marlow Goods").expect("the demo listing");
    let body = json!({
        "shipTo": { "name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202" },
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = node.request("POST", "/checkout", &body.to_string());
    assert_eq!(status, 200, "{placed}");
    placed["orders"][0].clone()
}
