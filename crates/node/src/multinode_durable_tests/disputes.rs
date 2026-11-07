//! Disputes from either party, durable cases and rulings, the M3 fee on
//! the vendor claim, the M6 limits, and a moderator on compact block filters.

use std::sync::atomic::Ordering;

use openbazaar_escrow::{SignedClaim, ANCHOR_VALUE};
use serde_json::{json, Value};

use crate::escrow_fixture::moderator_record;
use crate::escrow_limits::PER_ESCROW;
use crate::escrow_mail::unhex;
use crate::harness::{fund_node_wallet, order, request, Market};
use crate::multinode_tests::{funded, funding_tx, offered, sale, script_of, spends};
use crate::network::NetworkMode;
use crate::{MODERATOR_FEE_SATS, SETTLED};

fn funded_market() -> (Market, String, Value) {
    let market = Market::new();
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let (id, _) = offered(&market);
    let after = funded(&market, &id);
    market.mine();
    (market, id, after["escrow"].clone())
}

fn cases(market: &Market) -> Vec<Value> {
    let (_, value) = request(&market.moderator.node, "GET", "/moderator/disputes", "");
    value["disputes"].as_array().unwrap().clone()
}

fn rule(market: &Market, favor: &str) -> String {
    let id = cases(market)[0]["id"].as_str().unwrap().to_string();
    let path = format!("/moderator/disputes/{id}/rule");
    request(&market.moderator.node, "POST", &path, &json!({ "favor": favor }).to_string()).0
}

#[test]
fn a_dispute_and_its_ruling_survive_moderator_and_buyer_restarts() {
    let (mut market, id, _) = funded_market();
    let chain = market.chain.clone();
    let (status, body) = request(&market.buyer.node, "POST", &format!("/orders/{id}/dispute"), r#"{"memo": "Never arrived."}"#);
    assert!(status.contains("200"), "{status} {body}");
    market.moderator.restart(&chain, true);
    assert_eq!(market.deliver(), 1, "the letter waited in the moderator's inbox");
    market.moderator.restart(&chain, true);
    assert_eq!(cases(&market).len(), 1, "the case is on disk");

    assert!(rule(&market, "buyer").contains("200"));
    market.moderator.restart(&chain, true);
    assert!(rule(&market, "buyer").contains("409"), "one ruling, across a restart");
    assert_eq!(cases(&market)[0]["ruled"], true);

    market.buyer.restart(&chain, true);
    assert_eq!(market.deliver(), 1, "the buyer restores its escrow and settles");
    assert_eq!(order(&market.buyer.node, &id)["status"], SETTLED);
}

#[test]
fn the_vendor_disputes_and_a_vendor_ruling_settles_for_it() {
    let (market, id, escrow) = funded_market();
    let funding = escrow["fundingSats"].as_u64().unwrap();
    let fee = escrow["spendFeeSats"].as_u64().unwrap();
    let (_, outpoint) = funding_tx(&market, &escrow);
    let (status, _) = request(&market.buyer.node, "POST", &format!("/orders/{id}/dispute"), r#"{"memo": "Never arrived."}"#);
    assert!(status.contains("200"));
    assert_eq!(market.deliver(), 1, "the buyer's letter");
    let sale_id = sale(&market.vendor.node)["id"].as_str().unwrap().to_string();
    let path = format!("/escrow/sales/{sale_id}/dispute");
    let (status, body) = request(&market.vendor.node, "POST", &path, r#"{"memo": "Delivered; signed for."}"#);
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(sale(&market.vendor.node)["stage"], "disputed");
    assert_eq!(market.deliver(), 1, "the vendor's letter, signed in its own role");
    let favors: Vec<Value> = cases(&market).iter().map(|case| case["favors"].clone()).collect();
    assert_eq!(favors, vec![json!("buyer"), json!("vendor")]);
    assert!(rule(&market, "vendor").contains("200"));
    assert!(rule(&market, "buyer").contains("409"), "one ruling per escrow, whichever branch");
    assert_eq!(market.deliver(), 1, "the vendor settles with the ruling");

    let sold = sale(&market.vendor.node);
    assert_eq!(sold["stage"], "settled", "{sold}");
    let outcome = market.chain.broadcasts().last().unwrap().clone();
    assert_eq!(sold["settleTxid"], json!(outcome.compute_txid().to_string()));
    assert!(spends(&outcome, outpoint));
    assert_eq!(outcome.output[0].script_pubkey, script_of(&escrow["vendorPayout"]));
    assert_eq!(
        outcome.output[0].value.to_sat(),
        funding - fee - ANCHOR_VALUE.to_sat() - MODERATOR_FEE_SATS
    );
    assert_eq!(outcome.output[1].value.to_sat(), MODERATOR_FEE_SATS);
}

/// M3, option (a): the claim pays exactly what a vendor-wins ruling pays,
/// so a vendor gains nothing by waiting out a dispute instead of asking.
#[test]
fn m3_the_vendor_claim_pays_the_moderator_fee_like_a_vendor_ruling() {
    let (market, _, escrow) = funded_market();
    let funding = escrow["fundingSats"].as_u64().unwrap();
    let fee = escrow["spendFeeSats"].as_u64().unwrap();
    let claim = SignedClaim::from_bytes(&unhex(sale(&market.vendor.node)["claim"].as_str().unwrap()).unwrap()).unwrap();
    let claim = claim.transaction();
    let outputs: Vec<u64> = claim.output.iter().map(|out| out.value.to_sat()).collect();
    assert_eq!(claim.output[0].script_pubkey, script_of(&escrow["vendorPayout"]));
    assert_eq!(outputs[0], funding - fee - ANCHOR_VALUE.to_sat() - MODERATOR_FEE_SATS);
    assert!(outputs.contains(&MODERATOR_FEE_SATS), "{outputs:?}");
    assert!(outputs.contains(&ANCHOR_VALUE.to_sat()), "the claim can be bumped");
}

#[test]
fn m6_letters_past_the_per_escrow_limit_are_refused_before_review() {
    let (market, id, _) = funded_market();
    for n in 0..=PER_ESCROW {
        let memo = json!({ "memo": format!("Letter {n}") }).to_string();
        let (status, _) = request(&market.buyer.node, "POST", &format!("/orders/{id}/dispute"), &memo);
        assert!(status.contains("200"), "{status}");
        let handled = market.deliver();
        if n < PER_ESCROW {
            assert_eq!(handled, 1, "letter {n} is reviewed");
        } else {
            assert_eq!(handled, 0, "letter {n} is refused by the limit");
        }
    }
    assert_eq!(cases(&market).len(), 1, "one case per escrow and branch");
}

#[test]
fn a_moderator_on_block_filters_follows_the_funding_script_and_waits_for_the_scan() {
    let (market, id, escrow) = funded_market();
    let (funding, outpoint) = funding_tx(&market, &escrow);
    market.chain.0.filters.store(true, Ordering::Relaxed);
    let (status, _) = request(&market.buyer.node, "POST", &format!("/orders/{id}/dispute"), r#"{"memo": "Wrong item."}"#);
    assert!(status.contains("200"));
    assert_eq!(market.deliver(), 0, "the filters are still scanning: the letter waits");
    assert!(cases(&market).is_empty());

    let horizon = moderator_record(NetworkMode::Testnet4).unwrap().policy.max_refund_horizon;
    let from = (escrow["refundHeight"].as_u64().unwrap() as u32).saturating_sub(horizon);
    let script = funding.output[outpoint.vout as usize].script_pubkey.clone();
    assert_eq!(*market.chain.0.followed.lock().unwrap(), vec![(script, from)]);

    assert_eq!(market.deliver(), 1, "reviewed once the scan is done");
    assert_eq!(cases(&market)[0]["outpoint"], json!(outpoint.to_string()));
}
