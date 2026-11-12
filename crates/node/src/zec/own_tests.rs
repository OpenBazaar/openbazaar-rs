//! The node's own store on Zcash: the opt-in through `PUT /zec/stores`, and its wallet on the mock chain.

use std::sync::Arc;

use openbazaar_zec::mock::MockChain as ZecChainMock;
use serde_json::{json, Value};

use super::own::{NotReady, NEEDS_INDEXER, TESTNET4_ONLY};
use super::tests::open;
use crate::harness::{open_node, request, unreachable_escrow, MockChain, TempDir};
use crate::network::NetworkMode;
use crate::seller_store_tests::open_store;
use crate::Node;

pub(super) fn mock(network: NetworkMode) -> Arc<ZecChainMock> {
    Arc::new(ZecChainMock::new(network.zcash()))
}

pub(super) fn switch(node: &Node, accept: bool) -> (String, Value) {
    let own = node.own_record().expect("the own store is published").name;
    request(node, "PUT", "/zec/stores", &json!({"seller": own, "accept": accept}).to_string())
}

/// Bit 2 of the rail-flags byte of the own store's signed record.
fn signs_zec(node: &Node) -> bool {
    let record = node.own_record().unwrap();
    record.verify().unwrap();
    let bytes = record.encode().unwrap();
    bytes[bytes.len() - 64 - 1] & 4 != 0
}

#[test]
fn the_own_store_opts_in_and_the_choice_is_signed_and_survives_a_restart() {
    let (dir, btc, zec) = (TempDir::new(), MockChain::new(), mock(NetworkMode::Testnet4));
    let node = open(&dir.0, &btc, &zec);
    open_store(&node);
    assert!(!signs_zec(&node));
    node.zec_sync_once().unwrap();
    let (status, stores) = switch(&node, true);
    assert!(status.contains("200"), "{status} {stores}");
    assert_eq!(stores["stores"][0], json!({"seller": "Brook and Pine", "own": true, "acceptsZec": true, "problem": null}));
    assert!(signs_zec(&node));

    drop(node);
    let node = open(&dir.0, &btc, &zec);
    assert!(signs_zec(&node), "the choice survives a restart");
    let (status, stores) = switch(&node, false);
    assert!(status.contains("200") && stores["stores"][0]["acceptsZec"] == false, "{status} {stores}");
    assert!(!signs_zec(&node));
}

#[test]
fn a_locked_node_does_not_change_the_own_stores_choice() {
    let (dir, btc, zec) = (TempDir::new(), MockChain::new(), mock(NetworkMode::Testnet4));
    let node = open(&dir.0, &btc, &zec);
    open_store(&node);
    node.zec_sync_once().unwrap();
    node.lock.lock();
    let (status, value) = switch(&node, true);
    assert!(status.contains("423") && value["code"] == "locked", "{status} {value}");
    assert!(!signs_zec(&node));
}

#[test]
fn the_own_store_is_refused_on_mainnet_and_without_an_indexer() {
    let (dir, btc) = (TempDir::new(), MockChain::new());
    let main = open_node(&dir.0, NetworkMode::Mainnet, &btc, 1, unreachable_escrow());
    main.set_zec_chain(mock(NetworkMode::Mainnet)).unwrap();
    open_store(&main);
    let (status, value) = switch(&main, true);
    assert!(status.contains("403") && value["error"] == TESTNET4_ONLY, "{status} {value}");
    assert!(!signs_zec(&main));
    // Nor does a record signed some other way carry the flag there.
    let secret = main.store_secret(&main.own_store).expect("unlocked");
    let rails = crate::own_store::Rails { zec: true, ..crate::own_store::Rails::default() };
    main.publish_own(openbazaar_protocol::ContentRating::General, rails, &secret).unwrap();
    assert!(!signs_zec(&main));

    let dir = TempDir::new();
    let off = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    open_store(&off);
    let (status, value) = switch(&off, true);
    assert!(status.contains("409") && value["error"] == NEEDS_INDEXER, "{status} {value}");
    assert!(!signs_zec(&off));
}

#[test]
fn the_own_store_is_not_switched_on_before_it_could_take_an_order() {
    let (dir, btc, zec) = (TempDir::new(), MockChain::new(), mock(NetworkMode::Testnet4));
    let node = open(&dir.0, &btc, &zec);
    open_store(&node);
    let (status, value) = switch(&node, true);
    assert!(status.contains("409") && value["error"] == NotReady::NotSynced.vendor(), "the chain is not read yet: {status} {value}");
    node.zec_sync_once().unwrap();
    node.zec.rate.set_for_tests(None);
    let (status, value) = switch(&node, true);
    assert!(status.contains("409") && value["error"] == NotReady::NoPrice.vendor(), "{status} {value}");
    assert!(!signs_zec(&node));
    node.zec.rate.set_for_tests(Some(1_000_000));
    assert!(switch(&node, true).0.contains("200") && signs_zec(&node));

    // Started again without --zec-indexer: the signed flag stays, and Zcash settings says why orders are declined.
    drop(node);
    let node = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    let (_, stores) = request(&node, "GET", "/zec/stores", "");
    assert_eq!(stores["stores"][0], json!({"seller": "Brook and Pine", "own": true, "acceptsZec": true, "problem": NEEDS_INDEXER}));
    assert_eq!(node.zec_sale_refusal(), Some(NotReady::Off.buyer()));
}

#[test]
fn the_own_stores_wallet_is_the_node_phrases_and_sees_a_payment_while_locked() {
    let (dir, btc, zec) = (TempDir::new(), MockChain::new(), mock(NetworkMode::Testnet4));
    let node = open(&dir.0, &btc, &zec);
    let from_phrase = node.spend_seed(&node.buyer_wallet()).ok().expect("unlocked").zcash_view_key().unwrap();
    node.lock.lock();
    node.zec_sync_once().unwrap();
    let (index, address) = {
        let mut slot = super::guard(&node.zec.own);
        let wallet = slot.wallet.as_mut().expect("the viewing key is in the receive tier");
        assert!(wallet.keys().ufvk() == from_phrase.ufvk(), "ZIP 32 account 0 of the node's own phrase");
        assert!(wallet.scanned().is_some(), "it syncs with the demo store wallets");
        let issued = wallet.issue_address();
        slot.save(&node.zec.vault).unwrap();
        issued
    };
    zec.pay(&address, 250_000);
    zec.mine(1);
    node.zec_sync_once().unwrap();

    drop(node);
    let node = open(&dir.0, &btc, &zec);
    node.lock.lock();
    let mut slot = super::guard(&node.zec.own);
    let wallet = slot.wallet.as_mut().expect("the key came back while locked");
    let received = wallet.received(index);
    assert_eq!((received.total, received.txids.len()), (250_000, 1));
    assert!(wallet.issue_address().0 > index, "an address is not handed out twice");
}
