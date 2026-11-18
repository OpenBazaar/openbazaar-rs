//! A filer's on-chain fee payment that cannot confirm, on the three-node
//! harness: cleared on evidence (a double spend, or a chain source that
//! never had it after rebroadcasts), paid again with one of its coins, and
//! counted again if it confirms anyway. A confirmed payment is never cleared.

use std::sync::atomic::Ordering;

use bitcoin::absolute::LockTime;
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, Transaction, TxIn, TxOut};
use openbazaar_escrow::Party;
use serde_json::{json, Value};

use crate::chain::ChainSource;
use crate::escrow_mail::EscrowId;
use crate::harness::{fund_node_wallet, order, Market};
use crate::multinode_fee_tests::{filed, funded_with, quote_and_pay, FEE};
use crate::multinode_refund_tests::escrow_of;

/// A funded v5 escrow whose buyer filed and was asked for the fee.
fn asked(market: &Market) -> (String, EscrowId) {
    let (id, _) = funded_with(market, FEE);
    let escrow = escrow_of(&filed(market, &id));
    market.moderator.node.moderator_fee_steps();
    market.deliver();
    fund_node_wallet(&market.buyer.node, &market.chain, 50_000);
    (id, escrow)
}

fn pay(market: &Market, id: &str) {
    let (_, (status, paid)) = quote_and_pay(&market.buyer.node, &format!("/orders/{id}"));
    assert!(status.contains("200"), "{status} {paid}");
}

fn held(market: &Market, id: &str) -> Value {
    order(&market.buyer.node, id)["escrow"]["disputeFee"].clone()
}

fn payment_tx(market: &Market, escrow: &EscrowId) -> Transaction {
    let fee = market.buyer.node.filer_fee(Party::Buyer, escrow).unwrap();
    fee.payment.and_then(|payment| payment.tx).expect("an on-chain payment is stored with its transaction")
}

/// Mine a block and let the buyer's node watch its payment.
fn block(market: &Market) {
    market.mine();
    market.buyer.node.filer_fee_steps(market.chain.tip());
}

fn spending(outpoint: OutPoint, market: &Market) -> Transaction {
    let address = market.buyer.node.buyer_wallet().receive_address().unwrap();
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            ..TxIn::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: address.script_pubkey(),
        }],
    }
}

#[test]
fn a_payment_whose_coin_was_spent_elsewhere_is_cleared_and_paid_again() {
    let market = Market::new();
    let (id, escrow) = asked(&market);
    pay(&market, &id);
    let payment = payment_tx(&market, &escrow);
    let spent = payment.input[0].previous_output;
    let conflict = spending(spent, &market);
    market.chain.broadcast(&conflict).unwrap();
    market.chain.0.wallet_txs.lock().unwrap().push((conflict, true));
    block(&market);
    let fee = held(&market, &id);
    assert_eq!(
        (&fee["state"], &fee["clearedTxids"]),
        (&json!("requested"), &json!([payment.compute_txid().to_string()])),
        "a confirmed double spend clears it at once: {fee}"
    );

    fund_node_wallet(&market.buyer.node, &market.chain, 50_000);
    pay(&market, &id);
    let again = payment_tx(&market, &escrow);
    assert_ne!(again.compute_txid(), payment.compute_txid());
    assert!(again.input.iter().all(|input| input.previous_output != spent));
    assert_eq!(held(&market, &id)["state"], "paying");
}

#[test]
fn a_payment_the_network_never_saw_is_cleared_after_rebroadcasts_and_counts_again_if_it_confirms() {
    let market = Market::new();
    let (id, escrow) = asked(&market);
    market.chain.0.drop_broadcasts.store(true, Ordering::Relaxed);
    pay(&market, &id);
    let payment = payment_tx(&market, &escrow);
    for _ in 0..6 {
        block(&market);
        assert_eq!(held(&market, &id)["state"], "paying", "sent again, not cleared yet");
    }
    block(&market);
    let fee = held(&market, &id);
    assert_eq!(fee["clearedTxids"], json!([payment.compute_txid().to_string()]), "{fee}");
    assert_eq!(fee["state"], "requested");
    let wallet = market.buyer.node.buyer_wallet();
    assert!(
        payment.input.iter().all(|input| wallet.wallet.get_utxo(input.previous_output).is_some()),
        "its coins are the wallet's again"
    );
    drop(wallet);

    pay(&market, &id);
    let again = payment_tx(&market, &escrow);
    assert!(
        again
            .input
            .iter()
            .any(|input| payment.input.iter().any(|old| old.previous_output == input.previous_output)),
        "the new payment spends a coin of the cleared one, so only one can confirm"
    );

    market.chain.0.wallet_txs.lock().unwrap().push((payment.clone(), true));
    block(&market);
    let fee = held(&market, &id);
    assert_eq!(
        (&fee["state"], &fee["txid"]),
        (&json!("paying"), &json!(payment.compute_txid().to_string())),
        "the cleared payment confirmed after all, so it is the payment again: {fee}"
    );
    assert_eq!(fee["clearedTxids"], json!([again.compute_txid().to_string()]));
}

#[test]
fn a_confirmed_payment_is_never_cleared() {
    let market = Market::new();
    let (id, escrow) = asked(&market);
    market.chain.0.drop_broadcasts.store(true, Ordering::Relaxed);
    pay(&market, &id);
    let payment = payment_tx(&market, &escrow);
    market.chain.0.wallet_txs.lock().unwrap().push((payment.clone(), true));
    for _ in 0..10 {
        block(&market);
    }
    let fee = held(&market, &id);
    assert_eq!((&fee["state"], &fee["clearedTxids"]), (&json!("paying"), &json!([])), "{fee}");
    assert_eq!(fee["txid"], json!(payment.compute_txid().to_string()));
}
