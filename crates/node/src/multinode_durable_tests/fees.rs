//! The presigned fee comes from the chain source's estimate, and a stuck
//! presigned spend is bumped through its P2A anchor.

use bitcoin::OutPoint;
use openbazaar_escrow::{anchor_script, SignedRefund};
use serde_json::{json, Value};

use crate::escrow_fee::vsize_bound;
use crate::escrow_fixture::moderator_record;
use crate::escrow_mail::unhex;
use crate::harness::{fund_node_wallet, order, request, Market, PASSPHRASE};
use crate::multinode_tests::{funded, offered, sale};
use crate::network::NetworkMode;

#[test]
fn the_presigned_fee_follows_the_buyers_estimate_and_the_vendor_signs_it() {
    let market = Market::new();
    *market.chain.0.fee_rate.lock().unwrap() = 10.0;
    let (_, offered) = offered(&market);
    let vsize = vsize_bound(&moderator_record(NetworkMode::Testnet4).unwrap());
    assert_eq!(offered["escrow"]["spendFeeSats"], 10 * vsize);
    assert_eq!(sale(&market.vendor.node)["networkFeeSats"], 10 * vsize);
}

fn bump(market: &Market, body: Value) -> (String, Value) {
    request(&market.buyer.node, "POST", "/escrow/bump", &body.to_string())
}

#[test]
fn a_refund_is_bumped_through_its_anchor_within_the_callers_cap() {
    let market = Market::new();
    let buyer = &market.buyer.node;
    fund_node_wallet(buyer, &market.chain, 100_000);
    fund_node_wallet(buyer, &market.chain, 50_000);
    let (id, _) = offered(&market);
    let after = funded(&market, &id);
    let escrow = &after["escrow"];
    market.mine();
    let refund = SignedRefund::from_bytes(&unhex(escrow["refund"].as_str().unwrap()).unwrap()).unwrap();
    let refund = refund.transaction().clone();
    *market.chain.0.tip.lock().unwrap() = escrow["refundHeight"].as_u64().unwrap() as u32;

    let body = |max: u64, passphrase: &str| {
        json!({"escrow": escrow["id"], "spend": "refund", "feeRateSatVb": 25.0, "maxFeeSats": max, "passphrase": passphrase})
    };
    let (status, refused) = bump(&market, body(1, PASSPHRASE));
    assert!(status.contains("409"), "over the cap: {status} {refused}");
    assert_eq!(order(buyer, &id)["escrow"]["stage"], "refunded", "the refund itself went out: {refused}");

    let (status, bumped) = bump(&market, body(100_000, PASSPHRASE));
    assert!(status.contains("200"), "{status} {bumped}");
    let child = market.chain.broadcasts().last().unwrap().clone();
    assert_eq!(bumped["parentTxid"], json!(refund.compute_txid().to_string()));
    assert_eq!(bumped["childTxid"], json!(child.compute_txid().to_string()));
    let anchor = refund.output.iter().position(|out| out.script_pubkey == anchor_script()).unwrap();
    let anchor = OutPoint { txid: refund.compute_txid(), vout: anchor as u32 };
    assert!(child.input.iter().any(|input| input.previous_output == anchor));
    let rate = bumped["packageRateSatVb"].as_f64().unwrap();
    assert!(rate >= 25.0, "{rate}");
    assert!(bumped["childFeeSats"].as_u64().unwrap() <= 100_000);

    let (status, _) = bump(&market, body(100_000, "not the passphrase"));
    assert!(status.contains("400"), "the passphrase is asked again: {status}");
    buyer.lock.lock();
    let (status, _) = bump(&market, body(100_000, PASSPHRASE));
    assert!(status.contains("423"), "a locked wallet pays no bump: {status}");
}
