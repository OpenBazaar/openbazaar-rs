//! Mempool payments: shown in the wallet, never counted toward a balance or an order.

use openbazaar_xmr::{address_for, quote_payment, sign_payment, MoneroAddress, SpendKey, XmrChain, XmrWallet, CONFIRMATIONS};
use serde_json::{json, Value};

use super::tests::{piconero, setup, subaddress, xmr_order, Setup, NET, XMR};
use crate::harness::{order, request};

fn payer(seed: u8) -> (SpendKey, XmrWallet) {
    let key = SpendKey::from_seed_secret(&[seed; 32]);
    let wallet = XmrWallet::new(NET, key.view_keys());
    (key, wallet)
}

/// A funded wallet outside the node pays `amount` to `to`. The payment stays in the pool.
fn pay_from_outside(chain: &dyn XmrChain, (key, mut payer): (SpendKey, XmrWallet), to: &MoneroAddress, amount: u64) {
    payer.cover_unscanned(100);
    while !payer.sync_step(chain, 100).unwrap().caught_up() {}
    let quote = quote_payment(&payer, to, amount, chain.fee_rate().unwrap()).unwrap();
    let decoyed = chain.decoys(quote.inputs.clone(), chain.tip().unwrap()).unwrap();
    let signed = sign_payment(&payer, &quote, decoyed, &key, quote.fee * 2).unwrap();
    chain.publish(&signed.tx).unwrap();
}

fn wallet(node: &crate::Node) -> Value {
    request(node, "GET", "/xmr/wallet", "").1
}

#[test]
fn a_pool_payment_is_shown_as_incoming_but_pays_no_order_and_no_balance() {
    let Setup { node, xmr, btc: _btc, dir: _dir } = setup();
    let primary = address_for(wallet(&node)["primaryAddress"].as_str().unwrap(), NET).unwrap();
    let (id, payment) = xmr_order(&node);
    let due = piconero(&payment["amountPiconero"]);
    // Fund both payers first: a mined block takes whatever is in the pool.
    let (first, second) = (payer(60), payer(61));
    xmr.mine_to(&first.1.primary_address(), 5 * XMR);
    xmr.mine_to(&second.1.primary_address(), 5 * XMR);
    xmr.mine(CONFIRMATIONS);
    pay_from_outside(xmr.as_ref(), first, &primary, XMR / 2);
    pay_from_outside(xmr.as_ref(), second, &subaddress(&payment), due);
    node.xmr_sync_once().unwrap();

    let shown = wallet(&node);
    assert_eq!(shown["unconfirmed"]["incomingPiconero"], (XMR / 2).to_string(), "{shown}");
    assert_eq!(shown["unconfirmed"]["incomingXmr"], "0.5");
    assert_eq!(shown["unconfirmed"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(shown["unconfirmed"]["entries"][0]["subaddressIndex"], 0);
    assert_eq!(shown["balance"], json!({"unlockedPiconero": "0", "pendingPiconero": "0", "unlockedXmr": "0", "pendingXmr": "0"}));

    let seller = node.xmr.store("Marlow Goods").unwrap();
    assert_eq!(seller.pool.total(seller.wallet.as_ref().unwrap()), due, "the seller's wallet sees the order's payment in the pool");
    drop(seller);
    let waiting = order(&node, &id);
    assert_eq!(waiting["status"], "Awaiting payment", "a pool payment never pays an order: {waiting}");
    assert_eq!(waiting["payment"]["receivedPiconero"], "0");

    xmr.set_pool_offline(true);
    node.xmr_sync_once().expect("a failed pool request does not fail the round");
    assert!(wallet(&node)["syncError"].is_null());
    xmr.set_pool_offline(false);

    xmr.mine(1);
    node.xmr_sync_once().unwrap();
    let mined = wallet(&node);
    assert_eq!(mined["unconfirmed"]["incomingPiconero"], "0", "{mined}");
    assert!(mined["unconfirmed"]["entries"].as_array().unwrap().is_empty());
    assert_eq!(mined["balance"]["pendingPiconero"], (XMR / 2).to_string());
    assert_eq!(order(&node, &id)["status"], "Payment seen", "counted from its first confirmation");
}
