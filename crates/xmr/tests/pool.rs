//! Incoming payments seen in the mempool: shown, never counted.

use openbazaar_xmr::mock::MockChain;
use openbazaar_xmr::{quote_payment, sign_payment, PoolWatch, SpendKey, XmrChain, XmrNetwork, XmrWallet, CONFIRMATIONS};

const NET: XmrNetwork = XmrNetwork::Stagenet;
const XMR: u64 = 1_000_000_000_000;

fn wallet(seed: u8) -> (SpendKey, XmrWallet) {
    let spend = SpendKey::from_seed_secret(&[seed; 32]);
    let wallet = XmrWallet::new(NET, spend.view_keys());
    (spend, wallet)
}

fn sync(wallet: &mut XmrWallet, chain: &dyn XmrChain) {
    while !wallet.sync_step(chain, 100).unwrap().caught_up() {}
}

/// A funded buyer pays `amount` to `destination`; the payment sits in the pool. Returns the buyer and the txid.
fn pay_into_pool(chain: &MockChain, seed: u8, destination: &openbazaar_xmr::MoneroAddress, amount: u64) -> (XmrWallet, [u8; 32]) {
    let (key, mut buyer) = wallet(seed);
    sync(&mut buyer, chain);
    chain.mine_to(&buyer.primary_address(), 5 * XMR);
    chain.mine(CONFIRMATIONS);
    sync(&mut buyer, chain);
    let quote = quote_payment(&buyer, destination, amount, chain.fee_rate().unwrap()).unwrap();
    let decoyed = chain.decoys(quote.inputs.clone(), chain.tip().unwrap()).unwrap();
    let signed = sign_payment(&buyer, &quote, decoyed, &key, quote.fee * 2).unwrap();
    chain.publish(&signed.tx).unwrap();
    buyer.mark_spent(&signed.inputs, signed.txid);
    (buyer, signed.txid)
}

#[test]
fn a_pool_payment_to_a_subaddress_is_shown_but_not_counted() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(40);
    sync(&mut seller, &chain);
    let (index, address) = seller.issue_subaddress();
    let (_, txid) = pay_into_pool(&chain, 41, &address, XMR / 2);
    sync(&mut seller, &chain);

    let mut pool = PoolWatch::default();
    pool.refresh(&seller, &chain).unwrap();
    let incoming = pool.incoming(&seller);
    assert_eq!(incoming.len(), 1, "{incoming:?}");
    assert_eq!((incoming[0].txid, incoming[0].amount, incoming[0].subaddress), (txid, XMR / 2, index));
    assert_eq!(pool.total(&seller), XMR / 2);

    let balance = seller.balance();
    assert_eq!(balance.unlocked + balance.pending, 0, "the pool never reaches the balance");
    assert!(seller.spendable().is_empty());
    assert_eq!(seller.received(index).total, 0, "nor an order's subaddress");
}

#[test]
fn unchanged_pool_transactions_are_fetched_once() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(42);
    sync(&mut seller, &chain);
    let (_, address) = seller.issue_subaddress();
    pay_into_pool(&chain, 43, &address, XMR);
    let mut pool = PoolWatch::default();
    for _ in 0..3 {
        pool.refresh(&seller, &chain).unwrap();
    }
    assert_eq!(chain.pool_fetched(), 1, "an unchanged pool is not downloaded again");
    assert_eq!(pool.checked(), 1);

    // A newer subaddress was never scanned for, so the pool is checked again once.
    seller.issue_subaddress();
    pool.refresh(&seller, &chain).unwrap();
    pool.refresh(&seller, &chain).unwrap();
    assert_eq!(chain.pool_fetched(), 2);
}

#[test]
fn a_mined_payment_leaves_the_pool_view_and_counts_from_its_block() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(44);
    sync(&mut seller, &chain);
    let (index, address) = seller.issue_subaddress();
    pay_into_pool(&chain, 45, &address, XMR);
    let mut pool = PoolWatch::default();
    pool.refresh(&seller, &chain).unwrap();
    assert_eq!(pool.total(&seller), XMR);

    chain.mine(1);
    sync(&mut seller, &chain);
    assert_eq!(pool.total(&seller), 0, "once the wallet holds it, the stale pool entry is hidden");
    pool.refresh(&seller, &chain).unwrap();
    assert_eq!(pool.checked(), 0, "and the next refresh forgets it");
    assert_eq!(seller.received(index).total, XMR);
    assert_eq!(seller.balance().pending, XMR);
}

#[test]
fn the_payers_own_change_is_not_incoming() {
    let chain = MockChain::new(NET);
    let (_, seller) = wallet(46);
    let (buyer, _) = pay_into_pool(&chain, 47, &seller.primary_address(), XMR);
    let mut pool = PoolWatch::default();
    pool.refresh(&buyer, &chain).unwrap();
    assert!(pool.incoming(&buyer).is_empty(), "change from a spend this node made is not a payment");
    let mut seller_pool = PoolWatch::default();
    seller_pool.refresh(&seller, &chain).unwrap();
    assert_eq!(seller_pool.incoming(&seller)[0].subaddress, 0, "the primary address is index 0");
}

#[test]
fn a_failed_pool_request_leaves_the_block_scan_alone() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(48);
    sync(&mut seller, &chain);
    let (_, address) = seller.issue_subaddress();
    pay_into_pool(&chain, 49, &address, XMR);
    chain.set_pool_offline(true);
    let mut pool = PoolWatch::default();
    assert!(pool.refresh(&seller, &chain).is_err());
    assert_eq!(pool.total(&seller), 0);
    chain.mine(1);
    sync(&mut seller, &chain);
    assert_eq!(seller.balance().pending, XMR, "blocks are still scanned");
}
