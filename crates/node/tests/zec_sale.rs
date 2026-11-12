//! A Zcash sale between two real `ob-node` processes on loopback.
//!
//! The in-process tests (`zec/sale_tests.rs`) cover the same sale and step
//! the sync and the mail by hand. Here the binaries' own loops do it: the
//! seller's node reads a stand-in indexer through its real gRPC client and
//! prices from `--fixed-rates`; the buyer's node has no indexer and no ZEC
//! price, and learns everything from the seller's replies.

#[allow(dead_code)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::esplora::Esplora;
use common::{wait_for, RealNode};
use openbazaar_zec::mock::indexer::{indexer, serve};
use openbazaar_zec::mock::MockChain;
use openbazaar_zec::{ZecNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

fn ok(node: &RealNode, method: &str, path: &str, body: Value) -> Value {
    let (status, value) = node.request(method, path, &body.to_string());
    // `/setup` answers with the new wallet's phrase, so a failure there is not printed.
    assert_eq!(status, 200, "{} {method} {path}: {}", node.name, if path == "/setup" { Value::Null } else { value.clone() });
    value
}

#[test]
fn two_real_nodes_trade_in_zcash_and_only_the_seller_has_an_indexer() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let zec = Arc::new(MockChain::new(ZecNetwork::Testnet));
    let indexer_url = format!("http://{}", serve(&runtime, indexer(&zec, "v0.6.0")));
    let (btc, limit) = (Esplora::start(), Duration::from_secs(60));
    let seller = RealNode::start("seller", &btc.url, &["--zec-indexer", &indexer_url, "--fixed-rates", "BTCUSD=100000,ZECUSD=50"], limit);
    let buyer = RealNode::start("buyer", &btc.url, &[], limit);
    for node in [&seller, &buyer] {
        ok(node, "POST", "/setup", json!({ "passphrase": "correct horse battery" }));
    }
    ok(&seller, "PUT", "/store/role", json!({ "role": "seller" }));
    ok(&seller, "PUT", "/store/profile", json!({ "name": "Brook and Pine", "shipsTo": ["US"] }));
    let mug = json!({ "title": "Hand-thrown mug", "description": "Stoneware.", "category": "Home", "kind": "physical", "shipFrom": "Portland", "stock": 5, "currency": "SAT", "price": 25_000, "shipping": 3_000, "contentRating": "general" });
    let listing = ok(&seller, "POST", "/store/listings", mug);
    // The store is switched on only once its wallet has read the chain.
    wait_for("the seller's store to take Zcash", limit, || match seller.request("PUT", "/zec/stores", &json!({ "seller": "Brook and Pine", "accept": true }).to_string()) {
        (200, _) => Ok(()),
        (409, refused) => Err(refused.to_string()),
        (status, other) => panic!("PUT /zec/stores: {status} {other}"),
    });

    ok(&buyer, "POST", "/follows", json!({ "url": seller.url }));
    wait_for("the buyer's copy of the listing, with Zcash offered", limit, || {
        let (_, catalog) = buyer.request("GET", "/catalog", "");
        let found = catalog["listings"].as_array().is_some_and(|all| all.iter().any(|item| item["hash"] == listing["hash"] && item["acceptsZec"] == true));
        found.then_some(()).ok_or_else(|| catalog["zec"].to_string())
    });
    let body = json!({
        "shipTo": { "name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202" },
        "packages": [{ "settlement": "direct", "rail": "zec", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let placed = ok(&buyer, "POST", "/checkout", body);
    let path = format!("/orders/{}", placed["orders"][0]["orderId"].as_str().unwrap());
    let both_when = |step: &str, status: &str, confirmations: u64| {
        wait_for(step, limit, || {
            let (order, sales) = (buyer.request("GET", &path, "").1, seller.request("GET", "/sales", "").1);
            let ready = |of: &Value| of["status"] == status && of["confirmations"] == confirmations;
            if ready(&order) && ready(&sales["sales"][0]) { Ok(order) } else { Err(format!("{order} {sales}")) }
        })
    };

    // The seller's terms: 28,000 sats at its fixed rate of 50,000 sats per ZEC is 0.56 ZEC.
    let terms = both_when("the seller's terms", "Awaiting payment", 0);
    let payment = &terms["payment"];
    assert_eq!((&payment["rail"], &payment["satsPerZec"], &payment["amountZatoshi"], &terms["totalSats"]), (&json!("zec"), &json!(50_000), &json!("56000000"), &json!(28_000)), "{terms}");
    zec.pay(payment["zcashAddress"].as_str().unwrap(), 56_000_000);
    zec.mine(CONFIRMATIONS - 1);
    let seen = both_when("both nodes to see the payment at 29 confirmations", "Payment seen", CONFIRMATIONS - 1);
    assert_eq!(seen["payment"]["receivedZatoshi"], "56000000", "{seen}");
    zec.mine(1);
    let paid = both_when("both nodes to call it paid", "Paid", CONFIRMATIONS);
    assert_eq!(paid["remote"]["sellerSawPayment"], true, "{paid}");
    let (_, backend) = buyer.request("GET", "/network", "");
    assert_eq!(backend["zecBackend"]["enabled"], false, "the buyer's node never had an indexer");
}
