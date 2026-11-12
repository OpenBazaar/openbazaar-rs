//! Payments that do not arrive whole, or do not stay mined, on the nodes of
//! [`crate::multinode_seller_tests`] and [`crate::multinode_tests`]: a part
//! payment as the seller's node sees it, and a block that held a payment or
//! an escrow funding replaced before the nodes called it final.

use serde_json::Value;

use crate::harness::{fund_node_wallet, order, pay, request, Market};
use crate::multinode_seller_tests::Pair;
use crate::multinode_tests::{funded, offered, sale};
use crate::watch::{AWAITING_PAYMENT, COMPLETED, FUNDED, FUNDING_SEEN, PAID, PAYMENT_SEEN, RELEASED, SHIPPED};

/// One direct order the seller has answered. Returns the buyer's order id.
fn ordered(pair: &Pair) -> String {
    let id = pair.checkout(1)["orderId"].as_str().unwrap().to_string();
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, &id)["status"], AWAITING_PAYMENT);
    id
}

/// The seller's sale and the buyer's order: the two records of one payment.
fn both(pair: &Pair, id: &str) -> [Value; 2] {
    [pair.sales()[0].clone(), order(&pair.buyer.node, id)]
}

fn assert_seen(record: &Value, status: &str, received: u64, confirmations: u64) {
    assert_eq!(record["status"], status, "{record}");
    assert_eq!((record["receivedSats"].as_u64(), record["confirmations"].as_u64()), (Some(received), Some(confirmations)), "{record}");
}

#[test]
fn the_seller_sees_a_part_payment_then_the_rest_and_the_sale_completes() {
    let pair = Pair::new();
    let id = ordered(&pair);
    let sold = pair.sales()[0].clone();
    let (address, due) = (sold["address"].as_str().unwrap(), sold["totalSats"].as_u64().unwrap());

    pair.chain.pay_external(address, due - 10_000);
    pair.mine();
    for record in both(&pair, &id) {
        assert_seen(&record, AWAITING_PAYMENT, due - 10_000, 1);
    }

    pair.chain.pay_external(address, 10_000);
    pair.seller.node.watch_once().unwrap();
    assert_seen(&pair.sales()[0], PAYMENT_SEEN, due, 0);
    pair.mine();
    pair.deliver();
    for record in both(&pair, &id) {
        assert_seen(&record, PAID, due, 1);
        assert_eq!(record["txids"].as_array().unwrap().len(), 2, "{record}");
    }
    assert_eq!(order(&pair.buyer.node, &id)["remote"]["sellerSawPayment"], true);

    // From here it is a fully paid order.
    let fulfill = format!("/sales/{}/fulfill", sold["id"].as_str().unwrap());
    let (status, shipped) = request(&pair.seller.node, "POST", &fulfill, r#"{"tracking":"1Z"}"#);
    assert!(status.contains("200"), "{status} {shipped}");
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, &id)["status"], SHIPPED);
    let (status, done) = request(&pair.buyer.node, "POST", &format!("/orders/{id}/complete"), "");
    assert!(status.contains("200"), "{status} {done}");
    assert_eq!(done["status"], COMPLETED);
    assert_eq!(pair.sales()[0]["status"], COMPLETED);
}

#[test]
fn a_direct_payment_whose_block_is_replaced_before_paid_waits_to_be_mined_again() {
    let pair = Pair::with_confirmations(2);
    let id = ordered(&pair);
    let due = pair.sales()[0]["totalSats"].as_u64().unwrap();
    fund_node_wallet(&pair.buyer.node, &pair.chain, 100_000);
    let (status, sent) = pay(&pair.buyer.node, &id, "{}");
    assert!(status.contains("202"), "{status} {sent}");
    let seen = |confirmations| {
        for record in both(&pair, &id) {
            assert_seen(&record, PAYMENT_SEEN, due, confirmations);
        }
    };
    pair.mine();
    seen(1);

    pair.chain.reorg();
    pair.seller.node.watch_once().unwrap();
    pair.buyer.node.watch_once().unwrap();
    seen(0);
    pair.mine();
    seen(1);
    pair.mine();
    for record in both(&pair, &id) {
        assert_seen(&record, PAID, due, 2);
    }
}

#[test]
fn an_escrow_funding_whose_block_is_replaced_before_funded_waits_to_be_mined_again() {
    let market = Market::with_confirmations(2);
    let buyer = &market.buyer.node;
    fund_node_wallet(buyer, &market.chain, 100_000);
    let (id, _) = offered(&market);
    funded(&market, &id);
    let seen = |status: &str, confirmations: u64| {
        let now = order(buyer, &id);
        assert_eq!((now["status"].as_str(), now["confirmations"].as_u64()), (Some(status), Some(confirmations)), "{now}");
    };
    market.mine();
    seen(FUNDING_SEEN, 1);

    market.chain.reorg();
    buyer.watch_once().unwrap();
    seen(FUNDING_SEEN, 0);
    let release = format!("/orders/{id}/release");
    let (status, early) = request(buyer, "POST", &release, "{}");
    assert!(status.contains("409"), "no release on a funding that is not final: {status} {early}");
    market.mine();
    seen(FUNDING_SEEN, 1);
    market.mine();
    seen(FUNDED, 2);

    let (status, released) = request(buyer, "POST", &release, "{}");
    assert!(status.contains("200"), "{status} {released}");
    assert_eq!(released["status"], RELEASED);
    assert_eq!(market.deliver(), 1, "the vendor completes the release");
    assert_eq!(sale(&market.vendor.node)["stage"], "released");
}
