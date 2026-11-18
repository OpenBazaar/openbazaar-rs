//! The store's own fee address: paid from outside the wallet, renewed after
//! the epoch, and never paid twice while a fee is still confirming.

use bitcoin::absolute::LockTime;
use bitcoin::transaction::Version;
use bitcoin::{Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use openbazaar_protocol::{secret_from_label, xonly, Admission, DirectoryTerms, SeedDocument, EPOCH_BLOCKS, FEE_SATS};

use crate::admission::{fee_confirmed, store_fee_address};
use crate::chain::ChainSource;
use crate::harness::{self, MockChain, TempDir};
use crate::network::NetworkMode;
use crate::store_profile::Role;
use crate::Node;

fn seller(dir: &TempDir, chain: &MockChain) -> (Node, SeedDocument) {
    let node = harness::open_node(&dir.0, NetworkMode::Testnet4, chain, 1, harness::unreachable_escrow());
    let secret = secret_from_label("store-fee-address-head");
    let head = SeedDocument::sign(5, vec![format!("{}.onion", "a".repeat(56))], DirectoryTerms::standard(xonly(&secret)), None, &secret).unwrap();
    node.seed_replica.lock().unwrap().accept_document(&head.encode().unwrap(), &xonly(&secret)).unwrap();
    node.set_role(Role::Seller).unwrap();
    (node, head)
}

fn fee_address(node: &Node, head: &SeedDocument) -> Address {
    store_fee_address(&head.schedule.at(0).unwrap().terms.fee_key, node.own_store.as_bytes(), NetworkMode::Testnet4.bitcoin()).unwrap()
}

/// Someone outside the node, an exchange or a swap, pays `script`.
fn pay_from_outside(chain: &MockChain, script: ScriptBuf, sats: u64) -> Transaction {
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint { txid: bitcoin::Txid::from_raw_hash(bitcoin::hashes::Hash::all_zeros()), vout: 7 },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut { value: Amount::from_sat(sats), script_pubkey: script }],
    };
    chain.broadcast(&tx).unwrap();
    tx
}

fn queued_admission(node: &Node) -> Option<Admission> {
    node.seed_replica.lock().unwrap().read_outbox("admission").map(|bytes| Admission::decode(&bytes).unwrap())
}

fn fee_route(node: &Node) -> serde_json::Value {
    let (status, value) = harness::request(node, "GET", "/directory/fee", "");
    assert!(status.contains("200"), "{status} {value}");
    value
}

#[test]
fn a_payment_from_outside_the_wallet_admits_the_store() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let (node, head) = seller(&dir, &chain);
    let address = fee_address(&node, &head);
    assert_eq!(fee_route(&node)["state"], "checking");

    let err = node.publish_admission().unwrap_err();
    assert!(err.contains("cannot pay"), "{err}");
    let route = fee_route(&node);
    assert_eq!(route["available"], true);
    assert_eq!(route["address"], address.to_string());
    assert_eq!(route["sats"], FEE_SATS);
    assert_eq!(route["state"], "unpaid");

    pay_from_outside(&chain, address.script_pubkey(), FEE_SATS - 1);
    chain.mine();
    assert!(node.publish_admission().is_err(), "a short payment does not count, and the empty wallet still cannot pay");
    pay_from_outside(&chain, address.script_pubkey(), FEE_SATS);
    node.publish_admission().unwrap();
    assert_eq!(fee_route(&node)["state"], "confirming");
    assert!(queued_admission(&node).is_none());

    chain.mine();
    let height = chain.tip();
    node.publish_admission().unwrap();
    let admission = queued_admission(&node).expect("the outside payment admits the store");
    assert_eq!((admission.version, admission.store_id, admission.height), (2, *node.own_store.as_bytes(), height));
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &head, &admission));
    let route = fee_route(&node);
    assert_eq!(route["state"], "listed");
    assert_eq!(route["listedUntilHeight"], height + EPOCH_BLOCKS);
    assert_eq!(chain.broadcasts().len(), 2, "the node broadcast nothing of its own");
}

#[test]
fn the_node_pays_again_after_the_epoch_and_not_while_its_fee_is_unseen() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let (node, head) = seller(&dir, &chain);
    harness::fund_node_wallet(&node, &chain, 200_000);
    harness::fund_node_wallet(&node, &chain, 200_000);
    let script = fee_address(&node, &head).script_pubkey();

    node.publish_admission().unwrap();
    assert_eq!(chain.broadcasts().len(), 1);
    let seen = chain.0.by_script.lock().unwrap().remove(&script).expect("the fee is in the mempool");
    node.publish_admission().unwrap();
    assert_eq!(chain.broadcasts().len(), 1, "a server that has not seen the node's fee does not make it pay twice");
    chain.0.by_script.lock().unwrap().insert(script.clone(), seen);
    chain.mine();
    node.publish_admission().unwrap();
    let first_height = queued_admission(&node).expect("listed").height;

    *chain.0.tip.lock().unwrap() = first_height + EPOCH_BLOCKS + 1;
    node.publish_admission().unwrap();
    assert_eq!(chain.broadcasts().len(), 2, "an admission that left the epoch is paid again");
    node.publish_admission().unwrap();
    assert_eq!(chain.broadcasts().len(), 2);
    assert_eq!(fee_route(&node)["state"], "confirming");
    chain.mine();
    node.publish_admission().unwrap();
    let renewed = queued_admission(&node).unwrap();
    assert_eq!(renewed.height, chain.tip());
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &head, &renewed));
}

#[test]
fn the_fee_route_is_unavailable_on_mainnet() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = harness::open_node(&dir.0, NetworkMode::Mainnet, &chain, 1, harness::unreachable_escrow());
    let route = fee_route(&node);
    assert_eq!(route["available"], false);
    assert!(route["reason"].as_str().unwrap().contains("testnet4"));
}
