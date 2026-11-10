//! Wallets on the mock chain: payments into Ironwood, Orchard as outbound
//! only, order addresses, confirmations, reorgs, spends, restarts.

use std::collections::HashSet;

use openbazaar_zec::mock::{MockChain, BEFORE_IRONWOOD};
use openbazaar_zec::{Balance, Pool, Received, ViewKey, ZecChain, ZecNetwork, ZecWallet, CONFIRMATIONS};

const NET: ZecNetwork = ZecNetwork::Testnet;
const ZEC: u64 = 100_000_000;

fn wallet(seed: u8) -> (ViewKey, ZecWallet) {
    let keys = ViewKey::from_seed(&[seed; 64], NET).unwrap();
    (keys.clone(), ZecWallet::new(NET, keys))
}

fn sync(wallet: &mut ZecWallet, chain: &dyn ZecChain) {
    loop {
        if wallet.sync_step(chain, 50).unwrap().caught_up() {
            return;
        }
    }
}

/// A txid as wallets show it.
fn shown(txid: [u8; 32]) -> String {
    let mut reversed = txid;
    reversed.reverse();
    hex::encode(reversed)
}

#[test]
fn addresses_are_never_reused_across_restarts() {
    let (keys, mut wallet) = wallet(1);
    let mut seen = HashSet::new();
    for _ in 0..20 {
        let (index, address) = wallet.issue_address();
        assert!(index >= 1, "index 0 is the address a restored wallet shows first");
        assert!(seen.insert(address));
    }
    let mut again = ZecWallet::decode(&wallet.encode(), NET, keys).unwrap();
    for _ in 0..20 {
        let (index, address) = again.issue_address();
        assert!(index > 20);
        assert!(seen.insert(address), "address {index} was handed out twice");
    }
}

#[test]
fn an_ironwood_payment_reaches_its_order_and_confirms() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    let (other, _) = wallet.issue_address();
    let txid = chain.pay(&address, ZEC / 2);
    chain.mine(1);
    sync(&mut wallet, &chain);

    let received = wallet.received(index);
    assert_eq!(received.total, ZEC / 2);
    assert_eq!(received.confirmed, 0);
    assert_eq!(received.confirmations, Some(1));
    assert_eq!(received.txids, vec![shown(txid)]);
    assert_eq!(wallet.received(other), Received::default(), "the payment is matched to its own address only");
    assert_eq!(wallet.unspent().len(), 1, "the stranger's action in the same bundle is not ours");
    assert_eq!(wallet.unspent()[0].pool, Pool::Ironwood);

    chain.mine(CONFIRMATIONS - 2);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index).confirmations, Some(CONFIRMATIONS - 1));
    assert_eq!(wallet.received(index).confirmed, 0);
    assert_eq!(wallet.balance().pending, ZEC / 2);

    chain.mine(1);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index).confirmed, ZEC / 2);
    assert_eq!(wallet.balance().confirmed, ZEC / 2);
    assert_eq!(wallet.balance().pending, 0);
}

#[test]
fn orchard_notes_from_before_ironwood_are_outbound_only() {
    // A phrase that was paid in Orchard before NU6.3, restored on the node.
    let chain = MockChain::before_ironwood(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    assert_eq!(chain.payment_pool(), Pool::Orchard);
    chain.pay(&address, 3_000_000);
    chain.mine(BEFORE_IRONWOOD);
    assert_eq!(chain.payment_pool(), Pool::Ironwood, "the same receiver now receives in Ironwood");
    chain.pay(&address, 2_000_000);
    chain.mine(CONFIRMATIONS);
    sync(&mut wallet, &chain);

    assert_eq!(wallet.received(index).total, 2_000_000, "only Ironwood pays an order");
    assert_eq!(wallet.balance(), Balance { confirmed: 2_000_000, pending: 0, orchard: 3_000_000 });
    let pools: HashSet<_> = wallet.unspent().iter().map(|note| note.pool).collect();
    assert_eq!(pools, HashSet::from([Pool::Orchard, Pool::Ironwood]));

    // It can still leave Orchard.
    let orchard = wallet.unspent().into_iter().find(|note| note.pool == Pool::Orchard).unwrap();
    chain.spend(orchard.nullifier, Pool::Orchard);
    chain.mine(1);
    let step = wallet.sync_step(&chain, 50).unwrap();
    assert_eq!(step.spent, 1);
    assert_eq!(wallet.balance(), Balance { confirmed: 2_000_000, pending: 0, orchard: 0 });
}

#[test]
fn a_server_that_puts_new_value_in_orchard_is_not_believed() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    chain.pay_in_pool(&address, ZEC, Pool::Orchard);
    chain.mine(CONFIRMATIONS);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index), Received::default());
    assert_eq!(wallet.balance(), Balance::default());
    assert!(wallet.unspent().is_empty());
}

#[test]
fn payments_to_another_wallet_are_not_seen() {
    let chain = MockChain::new(NET);
    let (_, mut ours) = wallet(1);
    let (_, mut theirs) = wallet(2);
    sync(&mut ours, &chain);
    sync(&mut theirs, &chain);
    let (_, address) = theirs.issue_address();
    chain.pay(&address, ZEC);
    chain.mine(1);
    sync(&mut ours, &chain);
    sync(&mut theirs, &chain);
    assert!(ours.unspent().is_empty());
    assert_eq!(theirs.balance().pending, ZEC);
}

#[test]
fn two_payments_to_one_address_add_up_and_the_newest_sets_confirmations() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    let first = chain.pay(&address, 600_000);
    chain.mine(5);
    let second = chain.pay(&address, 400_000);
    chain.mine(1);
    sync(&mut wallet, &chain);
    let received = wallet.received(index);
    assert_eq!(received.total, 1_000_000);
    assert_eq!(received.confirmations, Some(1));
    assert_eq!(received.txids, vec![shown(first), shown(second)]);
}

#[test]
fn a_reorg_drops_a_payment_until_it_is_mined_again() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    chain.pay(&address, ZEC);
    chain.mine(3);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index).total, ZEC);

    chain.reorg(3);
    chain.mine(4);
    let step = wallet.sync_step(&chain, 50).unwrap();
    assert_eq!(step.rolled_back, 3);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index).total, 0, "the block that paid is gone");
    assert_eq!(wallet.scanned(), Some(chain.height()));

    chain.pay(&address, ZEC);
    chain.mine(1);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.received(index).total, ZEC);
}

#[test]
fn a_spend_is_seen_from_its_nullifier_and_undone_by_a_reorg() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    chain.pay(&address, ZEC);
    chain.mine(CONFIRMATIONS);
    sync(&mut wallet, &chain);
    let note = wallet.unspent().pop().unwrap();

    // Spent from the phrase restored in another wallet.
    chain.spend(note.nullifier, Pool::Ironwood);
    chain.mine(1);
    let step = wallet.sync_step(&chain, 50).unwrap();
    assert_eq!(step.spent, 1);
    assert!(wallet.unspent().is_empty());
    assert_eq!(wallet.balance().confirmed, 0);
    assert_eq!(wallet.received(index).total, ZEC, "what arrived for the order does not change");

    chain.reorg(1);
    chain.mine(2);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.unspent().len(), 1, "the spend was reorged away");
    assert_eq!(wallet.balance().confirmed, ZEC);
}

#[test]
fn the_wallet_file_round_trips_and_belongs_to_one_wallet() {
    let chain = MockChain::before_ironwood(NET);
    let (keys, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let (index, address) = wallet.issue_address();
    chain.pay(&address, 3_000_000);
    chain.mine(BEFORE_IRONWOOD);
    chain.pay(&address, ZEC);
    chain.mine(2);
    sync(&mut wallet, &chain);
    let ironwood = wallet.unspent().into_iter().find(|note| note.pool == Pool::Ironwood).unwrap();
    chain.spend(ironwood.nullifier, Pool::Ironwood);
    chain.pay(&address, 7);
    chain.mine(1);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.balance().orchard, 3_000_000);

    let mut again = ZecWallet::decode(&wallet.encode(), NET, keys.clone()).unwrap();
    assert_eq!(again.received(index), wallet.received(index));
    assert_eq!(again.balance(), wallet.balance());
    assert_eq!(again.unspent(), wallet.unspent());
    assert_eq!((again.scanned(), again.tip()), (wallet.scanned(), wallet.tip()));
    chain.mine(1);
    sync(&mut again, &chain);
    assert_eq!(again.scanned(), Some(chain.height()), "a restored wallet picks up where it left off");

    let (other, _) = self::wallet(2);
    assert!(ZecWallet::decode(&wallet.encode(), NET, other).is_err());
    let mainnet = ViewKey::from_seed(&[1u8; 64], ZecNetwork::Mainnet).unwrap();
    assert!(ZecWallet::decode(&wallet.encode(), ZecNetwork::Mainnet, mainnet).is_err());
    assert!(ZecWallet::decode(b"{}", NET, keys).is_err());
}

#[test]
fn a_sync_merged_after_an_address_was_issued_keeps_both() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let mut copy = wallet.clone();
    let (index, address) = wallet.issue_address();
    chain.pay(&address, ZEC);
    chain.mine(1);
    sync(&mut copy, &chain);
    assert!(wallet.merge_sync(copy), "the copy never saw the address, and still found its payment");
    assert_eq!(wallet.received(index).total, ZEC);
    assert_eq!(wallet.issue_address().0, index + 1, "the address counter is not rolled back");
}

#[test]
fn an_offline_server_is_an_error_not_a_reset() {
    let chain = MockChain::new(NET);
    let (_, mut wallet) = wallet(1);
    sync(&mut wallet, &chain);
    let scanned = wallet.scanned();
    chain.mine(3);
    chain.set_offline(true);
    assert!(wallet.sync_step(&chain, 50).is_err());
    assert_eq!(wallet.scanned(), scanned);
    chain.set_offline(false);
    sync(&mut wallet, &chain);
    assert_eq!(wallet.scanned(), Some(chain.height()));
}

#[test]
fn the_first_sync_starts_a_little_before_the_tip() {
    let chain = MockChain::new(NET);
    let (_, mut seller) = wallet(1);
    let (old, old_address) = seller.issue_address();
    let (recent, recent_address) = seller.issue_address();
    chain.pay(&old_address, ZEC);
    chain.mine(150);
    chain.pay(&recent_address, ZEC);
    chain.mine(50);
    sync(&mut seller, &chain);
    assert_eq!(seller.scanned(), Some(chain.height()));
    assert_eq!(seller.received(recent).total, ZEC, "within the first sync's reach");
    assert_eq!(seller.received(old).total, 0, "older than a new wallet looks");
}
