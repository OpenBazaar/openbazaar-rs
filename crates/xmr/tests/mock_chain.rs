//! Wallets on the mock chain: receiving, confirmations, reorgs, and a real signed payment.

use std::collections::HashSet;

use openbazaar_xmr::mock::MockChain;
use openbazaar_xmr::{
    monero_uri, quote_payment, sign_payment, SendError, SpendKey, XmrChain, XmrNetwork, XmrWallet, CONFIRMATIONS,
};

const NET: XmrNetwork = XmrNetwork::Stagenet;
const XMR: u64 = 1_000_000_000_000;

fn wallet(seed: u8) -> (SpendKey, XmrWallet) {
    let spend = SpendKey::from_seed_secret(&[seed; 32]);
    let wallet = XmrWallet::new(NET, spend.view_keys());
    (spend, wallet)
}

fn sync(wallet: &mut XmrWallet, chain: &dyn XmrChain) {
    loop {
        if wallet.sync_step(chain, 100).unwrap().caught_up() {
            return;
        }
    }
}

#[test]
fn subaddresses_are_never_reused_across_restarts() {
    let (spend, mut wallet) = wallet(1);
    let mut seen = HashSet::new();
    for _ in 0..20 {
        let (_, address) = wallet.issue_subaddress();
        assert!(seen.insert(address.to_string()));
    }
    let mut again = XmrWallet::decode(&wallet.encode(), NET, spend.view_keys()).unwrap();
    for _ in 0..20 {
        let (index, address) = again.issue_subaddress();
        assert!(index > 20);
        assert!(seen.insert(address.to_string()), "subaddress {index} was handed out twice");
    }
    assert!(!seen.contains(&wallet.primary_address().to_string()));
}

#[test]
fn a_payment_is_seen_then_confirmed_after_ten_blocks() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(2);
    sync(&mut seller, &chain);
    let (index, address) = seller.issue_subaddress();
    let (other, _) = seller.issue_subaddress();

    let txid = chain.mine_to(&address, 3 * XMR);
    sync(&mut seller, &chain);
    let received = seller.received(index);
    assert_eq!(received.total, 3 * XMR);
    assert_eq!(received.confirmed, 0);
    assert_eq!(received.confirmations, Some(1));
    assert_eq!(received.txids, vec![hex::encode(txid)]);
    assert_eq!(seller.received(other).total, 0, "outputs are counted per subaddress");

    chain.mine(CONFIRMATIONS - 2);
    sync(&mut seller, &chain);
    assert_eq!(seller.received(index).confirmations, Some(CONFIRMATIONS - 1));
    assert_eq!(seller.received(index).confirmed, 0);
    chain.mine(1);
    sync(&mut seller, &chain);
    assert_eq!(seller.received(index).confirmed, 3 * XMR);
    assert_eq!(seller.balance().unlocked, 3 * XMR);
}

#[test]
fn partial_payments_add_up_at_one_subaddress() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(3);
    sync(&mut seller, &chain);
    let (index, address) = seller.issue_subaddress();
    chain.mine_to(&address, XMR);
    chain.mine_to(&address, XMR / 2);
    sync(&mut seller, &chain);
    let received = seller.received(index);
    assert_eq!(received.total, XMR + XMR / 2);
    assert_eq!(received.txids.len(), 2);
}

#[test]
fn a_reorg_drops_outputs_from_replaced_blocks() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(4);
    sync(&mut seller, &chain);
    let (index, address) = seller.issue_subaddress();
    chain.mine_to(&address, XMR);
    chain.mine(2);
    sync(&mut seller, &chain);
    assert_eq!(seller.received(index).total, XMR);
    chain.reorg(3);
    let step = seller.sync_step(&chain, 100).unwrap();
    assert!(step.rolled_back >= 3, "{step:?}");
    sync(&mut seller, &chain);
    assert_eq!(seller.received(index).total, 0);
}

#[test]
fn the_wallet_file_belongs_to_one_wallet() {
    let (_, wallet_a) = wallet(5);
    let (other, _) = wallet(6);
    assert!(XmrWallet::decode(&wallet_a.encode(), NET, other.view_keys()).is_err());
    assert!(XmrWallet::decode(&wallet_a.encode(), XmrNetwork::Mainnet, wallet_a.keys().clone()).is_err());
}

#[test]
fn a_buyer_pays_one_subaddress_and_the_seller_sees_it() {
    let chain = MockChain::new(NET);
    let (buyer_key, mut buyer) = wallet(7);
    let (_, mut seller) = wallet(8);
    sync(&mut buyer, &chain);
    sync(&mut seller, &chain);
    chain.mine_to(&buyer.primary_address(), 5 * XMR);
    chain.mine(CONFIRMATIONS);
    sync(&mut buyer, &chain);
    assert_eq!(buyer.balance().unlocked, 5 * XMR);

    let (index, destination) = seller.issue_subaddress();
    let amount = 1_234_567_890_123;
    assert_eq!(monero_uri(&destination, amount), format!("monero:{destination}?tx_amount=1.234567890123"));
    let quote = quote_payment(&buyer, &destination, amount, chain.fee_rate().unwrap()).unwrap();
    assert!(quote.fee > 0 && quote.fee < XMR / 100, "fee {}", quote.fee);
    let cap = quote.fee * 3 / 2;

    let decoyed = chain.decoys(quote.inputs.clone(), chain.tip().unwrap()).unwrap();
    let signed = sign_payment(&buyer, &quote, decoyed, &buyer_key, cap).unwrap();
    assert!(signed.fee <= cap);
    chain.publish(&signed.tx).unwrap();
    buyer.mark_spent(&signed.inputs, signed.txid);
    assert_eq!(buyer.balance().unlocked, 0, "spent outputs leave the balance at once");

    let again = chain.decoys(quote.inputs.clone(), chain.tip().unwrap()).unwrap();
    let double = sign_payment(&buyer, &quote, again, &buyer_key, cap).unwrap();
    assert!(chain.publish(&double.tx).is_err(), "the mock refuses a double spend");

    chain.mine(CONFIRMATIONS);
    sync(&mut seller, &chain);
    sync(&mut buyer, &chain);
    assert_eq!(seller.received(index).confirmed, amount);
    assert_eq!(seller.received(index).txids, vec![hex::encode(signed.txid)]);
    let balance = buyer.balance();
    assert_eq!(balance.unlocked + balance.pending, 5 * XMR - amount - signed.fee, "change comes back to the buyer");
    assert_eq!(chain.published().len(), 1);
}

#[test]
fn signing_refuses_a_fee_above_the_cap_and_a_foreign_key() {
    let chain = MockChain::new(NET);
    let (buyer_key, mut buyer) = wallet(9);
    sync(&mut buyer, &chain);
    chain.mine_to(&buyer.primary_address(), 2 * XMR);
    chain.mine(CONFIRMATIONS);
    sync(&mut buyer, &chain);
    let (_, destination) = wallet(10).1.issue_subaddress();
    let quote = quote_payment(&buyer, &destination, XMR, chain.fee_rate().unwrap()).unwrap();
    let decoyed = || chain.decoys(quote.inputs.clone(), chain.tip().unwrap()).unwrap();

    let too_low = sign_payment(&buyer, &quote, decoyed(), &buyer_key, quote.fee / 2);
    assert!(matches!(too_low, Err(SendError::FeeAboveCap { .. })));
    let stranger = SpendKey::from_seed_secret(&[11; 32]);
    assert!(matches!(sign_payment(&buyer, &quote, decoyed(), &stranger, quote.fee * 2), Err(SendError::WrongKey)));
}

#[test]
fn quotes_refuse_unconfirmed_funds_and_other_networks() {
    let chain = MockChain::new(NET);
    let (_, mut buyer) = wallet(12);
    sync(&mut buyer, &chain);
    chain.mine_to(&buyer.primary_address(), 2 * XMR);
    sync(&mut buyer, &chain);
    let (_, destination) = wallet(13).1.issue_subaddress();
    let fee_rate = chain.fee_rate().unwrap();
    assert!(matches!(quote_payment(&buyer, &destination, XMR, fee_rate), Err(SendError::NotEnough { unlocked: 0, .. })));
    let mainnet = SpendKey::from_seed_secret(&[13; 32]).view_keys().subaddress(XmrNetwork::Mainnet, 1);
    assert!(matches!(quote_payment(&buyer, &mainnet, XMR, fee_rate), Err(SendError::Destination(_))));
}
