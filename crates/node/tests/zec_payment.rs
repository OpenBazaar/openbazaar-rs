//! One real `ob-node` process takes a Zcash order for a demo store from
//! checkout to Paid.
//!
//! The in-process tests (`zec/tests.rs`) cover the same order on a mock chain
//! and step the sync by hand. Here the binary's own sync loop asks a stand-in
//! indexer on loopback, through its real gRPC client, and the price comes
//! from `--fixed-rates`, so no rate server is asked.

#[allow(dead_code)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{chain_stub, wait_for, RealNode};
use openbazaar_zec::mock::indexer::{indexer, serve};
use openbazaar_zec::mock::MockChain;
use openbazaar_zec::{ZecNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

const PASSPHRASE: &str = "correct horse battery";
const NOT_SYNCED: &str = "has not read the Zcash chain yet";

#[test]
fn a_real_node_takes_a_zcash_order_to_paid_at_thirty_confirmations() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let chain = Arc::new(MockChain::new(ZecNetwork::Testnet));
    let zec = format!("http://{}", serve(&runtime, indexer(&chain, "v0.6.0")));
    let limit = Duration::from_secs(60);
    let flags = ["--zec-indexer", &zec, "--fixed-rates", "BTCUSD=100000,ZECUSD=50"];
    let node = RealNode::start("buyer", &chain_stub(), &flags, limit);
    // The answer carries the new wallet's phrase, so it is not printed.
    let (status, _) = node.request("POST", "/setup", &json!({ "passphrase": PASSPHRASE }).to_string());
    assert_eq!(status, 200, "POST /setup");
    let (status, stores) = node.request("PUT", "/zec/stores", &json!({ "seller": "Marlow Goods", "accept": true }).to_string());
    assert_eq!(status, 200, "{stores}");
    let (_, rates) = node.request("GET", "/rates", "");
    assert_eq!((&rates["source"], &rates["zec"]["source"], &rates["zec"]["satsPerZec"]), (&json!("fixed"), &json!("fixed"), &json!(50_000)), "{rates}");

    // A Zcash order is refused until the store's wallet has read the chain once.
    let placed = wait_for("the first Zcash sync", limit, || match checkout(&node) {
        (200, placed) => Ok(placed["orders"][0].clone()),
        (400, refused) if refused["error"].as_str().is_some_and(|error| error.contains(NOT_SYNCED)) => Err(refused.to_string()),
        (status, other) => panic!("POST /checkout: {status} {other}"),
    });
    assert_eq!((&placed["status"], &placed["payment"]["satsPerZec"]), (&json!("Awaiting payment"), &json!(50_000)), "{placed}");
    let due: u64 = placed["payment"]["amountZatoshi"].as_str().unwrap().parse().unwrap();
    let path = format!("/orders/{}", placed["orderId"].as_str().unwrap());
    let order_when = |step: &str, ready: &dyn Fn(&Value) -> bool| {
        wait_for(step, limit, || {
            let (_, order) = node.request("GET", &path, "");
            if ready(&order) { Ok(order) } else { Err(order.to_string()) }
        })
    };

    chain.pay(placed["payment"]["zcashAddress"].as_str().unwrap(), due);
    chain.mine(CONFIRMATIONS - 1);
    let seen = order_when("the payment at 29 confirmations", &|order| order["confirmations"] == CONFIRMATIONS - 1);
    assert_eq!(seen["status"], "Payment seen", "thirty confirmations are required: {seen}");
    assert_eq!(seen["payment"]["receivedZatoshi"], due.to_string(), "{seen}");
    chain.mine(1);
    let paid = order_when("the order to be paid", &|order| order["status"] == "Paid");
    assert_eq!(paid["confirmations"], CONFIRMATIONS, "{paid}");
    assert_eq!(paid["txids"].as_array().unwrap().len(), 1, "{paid}");
    assert!(!node.data_dir().join("tor-client").exists(), "nothing opened a Tor client");
}

/// `POST /checkout` for one demo-store listing, paid directly in Zcash.
fn checkout(node: &RealNode) -> (u16, Value) {
    let (status, catalog) = node.request("GET", "/catalog", "");
    assert_eq!(status, 200, "{catalog}");
    let listings = catalog["listings"].as_array().unwrap();
    let listing = listings.iter().find(|item| item["title"] == "Paperback novel" && item["seller"] == "Marlow Goods").expect("the demo listing");
    let body = json!({
        "shipTo": { "name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202" },
        "packages": [{ "settlement": "direct", "rail": "zec", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    node.request("POST", "/checkout", &body.to_string())
}
