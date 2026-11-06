//! Buyer, vendor, and moderator nodes in one process, on separate data dirs
//! and loopback ports, sharing one offline chain. Every escrow message goes
//! over real loopback HTTP into the recipient's sealed mailbox.

use std::sync::atomic::Ordering;

use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{OutPoint, Transaction};
use openbazaar_escrow::{SignedRefund, SignedTerms, ANCHOR_VALUE};
use serde_json::{json, Value};

use crate::escrow_mail::unhex;
use crate::harness::{checkout, fund_node_wallet, order, pay, request, Market};
use crate::network::{address_for, NetworkMode};
use crate::{
    Node, AWAITING_FUNDING, AWAITING_TERMS, DISPUTED, ESCROW_SPEND_FEE_SATS, FUNDED,
    FUNDED_WITHOUT_REFUND, MODERATOR_FEE_SATS, RELEASED, SETTLED,
};

const SELLER: &str = "Marlow Goods";

fn script_of(text: &Value) -> bitcoin::ScriptBuf {
    address_for(text.as_str().unwrap(), NetworkMode::Testnet4)
        .unwrap()
        .script_pubkey()
}

fn sale(node: &Node) -> Value {
    let (status, value) = request(node, "GET", "/escrow/sales", "");
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!(value["sales"].as_array().unwrap().len(), 1, "{value}");
    value["sales"][0].clone()
}

/// Check out one escrow order and run the rounds until the vendor's terms are back.
fn offered(market: &Market) -> (String, Value) {
    let buyer = &market.buyer.node;
    let placed = checkout(buyer, &[("Paperback novel", SELLER, "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    assert_eq!(placed[0]["status"], AWAITING_TERMS);
    assert!(
        placed[0]["escrow"]["address"].is_null(),
        "no address before the vendor's terms"
    );
    assert!(placed[0]["escrow"].get("error").is_none(), "{}", placed[0]);
    assert_eq!(
        market.deliver(),
        2,
        "request to the vendor, offer back to the buyer"
    );

    let offered = order(buyer, &id);
    assert_eq!(offered["status"], AWAITING_FUNDING, "{offered}");
    let escrow = &offered["escrow"];
    assert_eq!(escrow["stage"], "offered");
    assert!(escrow["address"].as_str().unwrap().starts_with("tb1p"));
    assert_eq!(
        escrow["fundingSats"],
        offered["totalSats"].as_u64().unwrap() + ESCROW_SPEND_FEE_SATS
    );
    let deadline = market.chain.tip() + crate::escrow_vendor::DELIVERY_BLOCKS;
    assert_eq!(escrow["disputeDeadline"], deadline);
    assert!(
        escrow["refundHeight"].as_u64().unwrap() > escrow["vendorClaimHeight"].as_u64().unwrap()
    );

    let sold = sale(&market.vendor.node);
    assert_eq!(sold["address"], escrow["address"]);
    assert_eq!(sold["stage"], "offered");
    (id, offered)
}

/// Each node holds only its own per-order key.
fn assert_keys_are_split(market: &Market, escrow: &Value) {
    let terms = SignedTerms::decode(&unhex(escrow["terms"].as_str().unwrap()).unwrap()).unwrap();
    let terms = terms.terms();
    let public = |secret| PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize();
    let buyer_index = escrow["buyerKeyIndex"].as_u64().unwrap() as u32;
    let vendor_index = sale(&market.vendor.node)["vendorKeyIndex"]
        .as_u64()
        .unwrap() as u32;
    assert_eq!(
        terms.buyer,
        public(
            market
                .buyer
                .node
                .buyer_wallet()
                .seed
                .escrow_key(buyer_index)
        )
    );
    assert_eq!(
        terms.vendor,
        public(
            market
                .vendor
                .node
                .store_wallet(SELLER)
                .unwrap()
                .seed
                .escrow_key(vendor_index)
        )
    );
    for index in 0..4 {
        assert_ne!(
            terms.vendor,
            public(
                market
                    .buyer
                    .node
                    .store_wallet(SELLER)
                    .unwrap()
                    .seed
                    .escrow_key(index)
            )
        );
        assert_ne!(
            terms.buyer,
            public(market.vendor.node.buyer_wallet().seed.escrow_key(index))
        );
    }
}

/// Pay an offered escrow from the buyer's wallet and run the funding rounds.
fn funded(market: &Market, id: &str) -> Value {
    let buyer = &market.buyer.node;
    let (status, opened) = pay(buyer, id, "{}");
    assert!(status.contains("202"), "{status} {opened}");
    assert!(opened.get("payError").is_none(), "{opened}");
    assert_eq!(opened["escrow"]["stage"], "opened");
    assert!(
        market.chain.broadcasts().is_empty(),
        "nothing is signed before the vendor replies"
    );
    assert_eq!(
        market.deliver(),
        4,
        "open, reply, signatures, release nonce"
    );

    let after = order(buyer, id);
    assert_eq!(after["escrow"]["stage"], "funded", "{after}");
    assert_eq!(after["escrow"]["refundHeld"], true);
    assert_eq!(after["escrow"]["releaseReady"], true);
    assert_eq!(sale(&market.vendor.node)["stage"], "funded");
    assert_eq!(sale(&market.vendor.node)["claimHeld"], true);
    after
}

fn funding_tx(market: &Market, escrow: &Value) -> (Transaction, OutPoint) {
    let outpoint: OutPoint = escrow["outpoint"].as_str().unwrap().parse().unwrap();
    let tx = market
        .chain
        .broadcasts()
        .into_iter()
        .find(|tx| tx.compute_txid() == outpoint.txid)
        .expect("the funding was broadcast");
    (tx, outpoint)
}

fn spends(tx: &Transaction, outpoint: OutPoint) -> bool {
    tx.input.len() == 1 && tx.input[0].previous_output == outpoint
}

#[test]
fn escrow_released_to_the_vendor_across_three_nodes() {
    let market = Market::new();
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let (id, offered) = offered(&market);
    assert_keys_are_split(&market, &offered["escrow"]);
    let after = funded(&market, &id);
    let escrow = &after["escrow"];
    let funding = escrow["fundingSats"].as_u64().unwrap();

    let (funding_tx, outpoint) = funding_tx(&market, escrow);
    assert_eq!(after["payTxid"], json!(outpoint.txid.to_string()));
    let output = &funding_tx.output[outpoint.vout as usize];
    assert_eq!(output.script_pubkey, script_of(&escrow["address"]));
    assert_eq!(output.value.to_sat(), funding);
    assert!(
        funding_tx
            .input
            .iter()
            .all(|input| !input.sequence.is_rbf()),
        "the funding never signals RBF"
    );

    let refund =
        SignedRefund::from_bytes(&unhex(escrow["refund"].as_str().unwrap()).unwrap()).unwrap();
    let refund = refund.transaction();
    assert!(spends(refund, outpoint));
    assert_eq!(
        refund.lock_time.to_consensus_u32() as u64,
        escrow["refundHeight"].as_u64().unwrap()
    );
    assert_eq!(
        refund.output[0].script_pubkey,
        script_of(&escrow["buyerPayout"])
    );

    let (status, early) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/release"),
        "{}",
    );
    assert!(
        status.contains("409"),
        "release waits for confirmation: {status} {early}"
    );

    market.mine();
    assert_eq!(order(&market.buyer.node, &id)["status"], FUNDED);
    let (status, released) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/release"),
        "{}",
    );
    assert!(status.contains("200"), "{status} {released}");
    assert_eq!(released["status"], RELEASED);
    assert_eq!(market.deliver(), 1, "the vendor completes the release");

    let release = market.chain.broadcasts().last().unwrap().clone();
    assert!(spends(&release, outpoint));
    assert_eq!(release.output.len(), 1);
    assert_eq!(
        release.output[0].script_pubkey,
        script_of(&escrow["vendorPayout"])
    );
    assert_eq!(
        release.output[0].value.to_sat(),
        funding - ESCROW_SPEND_FEE_SATS
    );
    let sold = sale(&market.vendor.node);
    assert_eq!(sold["stage"], "released");
    assert_eq!(
        sold["releaseTxid"],
        json!(release.compute_txid().to_string())
    );

    market.mine();
    assert_eq!(
        order(&market.buyer.node, &id)["status"],
        RELEASED,
        "the watcher leaves a released escrow alone"
    );
    let (status, _) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "late"}"#,
    );
    assert!(status.contains("409"), "no dispute after a release");
}

#[test]
fn escrow_dispute_ruled_for_the_buyer() {
    let market = Market::new();
    fund_node_wallet(&market.buyer.node, &market.chain, 100_000);
    let (id, _) = offered(&market);
    let after = funded(&market, &id);
    let escrow = &after["escrow"];
    let funding = escrow["fundingSats"].as_u64().unwrap();
    let (_, outpoint) = funding_tx(&market, escrow);
    market.mine();

    let (status, disputed) = request(
        &market.buyer.node,
        "POST",
        &format!("/orders/{id}/dispute"),
        r#"{"memo": "The box arrived empty."}"#,
    );
    assert!(status.contains("200"), "{status} {disputed}");
    assert_eq!(disputed["status"], DISPUTED);
    assert_eq!(market.deliver(), 1, "the moderator reviews the letter");

    let moderator = &market.moderator.node;
    let (_, cases) = request(moderator, "GET", "/moderator/disputes", "");
    let case = &cases["disputes"][0];
    assert_eq!(case["favors"], "buyer");
    assert_eq!(case["memo"], "The box arrived empty.");
    assert_eq!(case["outpoint"], json!(outpoint.to_string()));
    let rule = format!("/moderator/disputes/{}/rule", case["id"].as_str().unwrap());
    let (status, _) = request(moderator, "POST", &rule, r#"{"favor": "vendor"}"#);
    assert!(
        status.contains("400"),
        "a buyer's letter cannot open the vendor branch"
    );
    let (status, ruled) = request(moderator, "POST", &rule, r#"{"favor": "buyer"}"#);
    assert!(status.contains("200"), "{status} {ruled}");
    assert_eq!(market.deliver(), 1, "the buyer settles with the ruling");

    let settled = order(&market.buyer.node, &id);
    assert_eq!(settled["status"], SETTLED, "{settled}");
    let outcome = market.chain.broadcasts().last().unwrap().clone();
    assert_eq!(
        settled["escrow"]["settleTxid"],
        json!(outcome.compute_txid().to_string())
    );
    assert!(spends(&outcome, outpoint));
    let winner = funding - ESCROW_SPEND_FEE_SATS - ANCHOR_VALUE.to_sat() - MODERATOR_FEE_SATS;
    assert_eq!(
        outcome.output[0].script_pubkey,
        script_of(&escrow["buyerPayout"])
    );
    assert_eq!(outcome.output[0].value.to_sat(), winner);
    assert_eq!(outcome.output[1].value.to_sat(), MODERATOR_FEE_SATS);

    let (status, _) = request(moderator, "POST", &rule, r#"{"favor": "buyer"}"#);
    assert!(status.contains("409"), "one ruling per escrow");
    market.mine();
    assert_eq!(order(&market.buyer.node, &id)["status"], SETTLED);
}

#[test]
fn a_failed_funding_broadcast_keeps_the_refund_and_retries_the_same_transaction() {
    let market = Market::new();
    let buyer = &market.buyer.node;
    fund_node_wallet(buyer, &market.chain, 100_000);
    let (id, _) = offered(&market);
    pay(buyer, &id, "{}");
    market.chain.0.fail_broadcast.store(true, Ordering::Relaxed);
    assert_eq!(
        market.deliver(),
        3,
        "the buyer's reply round fails at the broadcast"
    );

    let held = order(buyer, &id);
    assert_eq!(
        held["escrow"]["refundHeld"], true,
        "the refund is stored before the broadcast"
    );
    assert_eq!(held["escrow"]["stage"], "signed");
    assert_eq!(held["status"], AWAITING_FUNDING);
    assert!(held.get("payTxid").is_none());
    let stored: Transaction =
        deserialize_hex(held["escrow"]["fundingTx"].as_str().unwrap()).unwrap();

    let (status, paid) = pay(buyer, &id, "{}");
    assert!(status.contains("202"), "{status} {paid}");
    assert_eq!(
        paid["payTxid"],
        json!(stored.compute_txid().to_string()),
        "{paid}"
    );
    assert_eq!(market.chain.broadcasts().len(), 1);
    assert_eq!(
        market.chain.broadcasts()[0].compute_txid(),
        stored.compute_txid()
    );
    market.mine();
    assert_eq!(order(buyer, &id)["status"], FUNDED);
}

#[test]
fn escrow_funded_from_elsewhere_is_flagged_without_a_refund() {
    let market = Market::new();
    let buyer = &market.buyer.node;
    let (id, offered) = offered(&market);
    let address = offered["escrow"]["address"].as_str().unwrap();
    let funding = offered["escrow"]["fundingSats"].as_u64().unwrap();

    market.chain.pay_external(address, funding - 10);
    market.mine();
    let short = order(buyer, &id);
    assert_eq!(short["status"], AWAITING_FUNDING);
    assert_eq!(short["receivedSats"], funding - 10);

    market.chain.pay_external(address, funding);
    market.mine();
    let flagged = order(buyer, &id);
    assert_eq!(flagged["status"], FUNDED_WITHOUT_REFUND);
    assert_eq!(flagged["escrow"]["refundHeld"], false);
}
