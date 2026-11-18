//! A head that schedules new fee terms: a store that paid stays listed for
//! its epoch, a payment is judged by the entry at its height, and the seller's
//! node pays across a boundary without losing its listing.

use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::SecretKey;
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use openbazaar_protocol::{
    secret_from_label, xonly, Admission, DirectoryTerms, SeedDocument, TermsEntry, TermsSchedule, EPOCH_BLOCKS, FEE_SATS, NOTICE_BLOCKS,
};

use crate::admission::{fee_confirmed, listed_until, payment_terms, store_fee_address, GUARD_BLOCKS};
use crate::chain::ChainSource;
use crate::harness::{self, MockChain, TempDir};
use crate::network::NetworkMode;
use crate::store_profile::Role;
use crate::Node;

const RAISED: u64 = 30_000;

fn operator() -> SecretKey {
    secret_from_label("fee-schedule-head")
}

fn standard() -> DirectoryTerms {
    DirectoryTerms::standard(xonly(&operator()))
}

fn head(sequence: u64, entries: Vec<TermsEntry>) -> SeedDocument {
    let onion = format!("{}.onion", "a".repeat(56));
    SeedDocument::sign(sequence, vec![onion], TermsSchedule::new(entries).unwrap(), None, &operator()).unwrap()
}

fn raise_at(start: u32, terms: DirectoryTerms) -> TermsEntry {
    TermsEntry { start_height: start, announced_height: start - NOTICE_BLOCKS, terms }
}

fn raised(sequence: u64, start: u32) -> SeedDocument {
    head(sequence, vec![TermsEntry::genesis(standard()), raise_at(start, DirectoryTerms { fee_sats: RAISED, ..standard() })])
}

fn seller(dir: &TempDir, chain: &MockChain, document: &SeedDocument) -> Node {
    let node = harness::open_node(&dir.0, NetworkMode::Testnet4, chain, 1, harness::unreachable_escrow());
    hold(&node, document);
    node.set_role(Role::Seller).unwrap();
    node
}

fn hold(node: &Node, document: &SeedDocument) {
    node.seed_replica.lock().unwrap().accept_document(&document.encode().unwrap(), &xonly(&operator())).unwrap();
}

fn fee_script(store: &[u8; 32]) -> ScriptBuf {
    store_fee_address(&standard().fee_key, store, NetworkMode::Testnet4.bitcoin()).unwrap().script_pubkey()
}

fn pay_from_outside(chain: &MockChain, script: ScriptBuf, sats: u64) -> Transaction {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint { txid: bitcoin::Txid::all_zeros(), vout: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::default(),
        }],
        output: vec![TxOut { value: Amount::from_sat(sats), script_pubkey: script }],
    };
    chain.broadcast(&tx).unwrap();
    tx
}

/// Mine `tx` into block `height`.
fn confirm_at(chain: &MockChain, height: u32) {
    *chain.0.tip.lock().unwrap() = height - 1;
    chain.mine();
}

fn admit(secret: &SecretKey, tx: &Transaction, height: u32) -> Admission {
    Admission::sign(xonly(secret), tx.compute_txid().to_byte_array(), 0, height, secret).unwrap()
}

fn queued_admission(node: &Node) -> Option<Admission> {
    node.seed_replica.lock().unwrap().read_outbox("admission").map(|bytes| Admission::decode(&bytes).unwrap())
}

fn fee_outputs(chain: &MockChain, node: &Node) -> Vec<u64> {
    let script = fee_script(node.own_store.as_bytes());
    chain.broadcasts().iter().flat_map(|tx| tx.output.iter()).filter(|out| out.script_pubkey == script).map(|out| out.value.to_sat()).collect()
}

#[test]
fn a_raise_keeps_every_paid_store_listed_until_its_epoch_ends() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = seller(&dir, &chain, &head(5, vec![TermsEntry::genesis(standard())]));
    pay_from_outside(&chain, fee_script(node.own_store.as_bytes()), FEE_SATS);
    chain.mine();
    node.publish_admission().unwrap();
    let admission = queued_admission(&node).expect("listed under the first terms");
    let paid_at = admission.height;

    let start = chain.tip() + NOTICE_BLOCKS;
    let next = raised(6, start);
    hold(&node, &next);
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &next, &admission), "a new head does not unlist a store that paid");
    *chain.0.tip.lock().unwrap() = paid_at + EPOCH_BLOCKS;
    assert!(start < chain.tip(), "the raise is in force");
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &next, &admission), "the old payment keeps its own epoch");
    harness::fund_node_wallet(&node, &chain, 200_000);
    node.publish_admission().unwrap();
    assert!(fee_outputs(&chain, &node).len() == 1, "the node does not pay again while its listing runs");

    *chain.0.tip.lock().unwrap() = paid_at + EPOCH_BLOCKS + 1;
    assert!(!fee_confirmed(&chain, NetworkMode::Testnet4, &next, &admission));
    node.publish_admission().unwrap();
    assert_eq!(fee_outputs(&chain, &node), vec![FEE_SATS, RAISED], "the renewal pays the raised fee");
}

#[test]
fn a_payment_is_judged_by_the_entry_at_its_confirming_height() {
    let chain = MockChain::new();
    let store = secret_from_label("schedule-store");
    let start = 5_000;
    let slower = DirectoryTerms { fee_sats: RAISED, fee_confs: 2, epoch_blocks: 4_032, ..standard() };
    let document = head(7, vec![TermsEntry::genesis(standard()), raise_at(start, slower)]);
    let script = fee_script(&xonly(&store));

    let before = pay_from_outside(&chain, script.clone(), FEE_SATS);
    confirm_at(&chain, start - 1);
    let before = admit(&store, &before, start - 1);
    let short = pay_from_outside(&chain, script.clone(), FEE_SATS);
    confirm_at(&chain, start);
    let short = admit(&store, &short, start);
    let full = pay_from_outside(&chain, script.clone(), RAISED);
    confirm_at(&chain, start + 1);
    let full = admit(&store, &full, start + 1);

    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &document, &before), "the block before the start keeps the old fee");
    assert!(!fee_confirmed(&chain, NetworkMode::Testnet4, &document, &short), "from the start the raised fee applies");
    assert!(!fee_confirmed(&chain, NetworkMode::Testnet4, &document, &full), "the new entry needs two confirmations");
    chain.mine();
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &document, &full));
    assert_eq!(listed_until(&document.schedule, start - 1), start - 1 + EPOCH_BLOCKS);
    assert_eq!(listed_until(&document.schedule, start + 1), start + 1 + 4_032, "the epoch runs from the payment under its own entry");

    let pruned = head(8, vec![raise_at(start, slower)]);
    assert!(!fee_confirmed(&chain, NetworkMode::Testnet4, &pruned, &before), "an admission under a dropped entry is not listed");
}

#[test]
fn inside_the_guard_the_node_pays_the_higher_fee() {
    let start = 5_000;
    let document = raised(9, start);
    assert_eq!(payment_terms(&document.schedule, start - GUARD_BLOCKS - 2).unwrap().fee_sats, FEE_SATS);
    assert_eq!(payment_terms(&document.schedule, start - GUARD_BLOCKS - 1).unwrap().fee_sats, RAISED);
    assert_eq!(payment_terms(&document.schedule, start - 1).unwrap().fee_sats, RAISED);
    let dropped = head(9, vec![TermsEntry::genesis(standard()), TermsEntry { start_height: start, announced_height: start - 288, terms: DirectoryTerms { fee_sats: 10_000, ..standard() } }]);
    assert_eq!(payment_terms(&dropped.schedule, start - 2).unwrap().fee_sats, FEE_SATS, "a drop inside the guard pays the old, higher fee");
    assert_eq!(payment_terms(&dropped.schedule, start - 1).unwrap().fee_sats, 10_000);

    let dir = TempDir::new();
    let chain = MockChain::new();
    *chain.0.tip.lock().unwrap() = start - 3;
    let node = seller(&dir, &chain, &document);
    harness::fund_node_wallet(&node, &chain, 200_000);
    node.publish_admission().unwrap();
    assert_eq!(fee_outputs(&chain, &node), vec![RAISED]);
    chain.mine();
    node.publish_admission().unwrap();
    let admission = queued_admission(&node).expect("the higher fee lists the store before the start");
    assert_eq!(admission.height, start - 2);
    assert!(fee_confirmed(&chain, NetworkMode::Testnet4, &document, &admission));
}

#[test]
fn an_old_fee_that_confirms_after_a_raise_is_paid_again() {
    let start = 5_000;
    let document = raised(10, start);
    let dir = TempDir::new();
    let chain = MockChain::new();
    *chain.0.tip.lock().unwrap() = start - 20;
    let node = seller(&dir, &chain, &document);
    harness::fund_node_wallet(&node, &chain, 200_000);
    harness::fund_node_wallet(&node, &chain, 200_000);
    node.publish_admission().unwrap();
    assert_eq!(fee_outputs(&chain, &node), vec![FEE_SATS]);
    node.publish_admission().unwrap();
    assert_eq!(fee_outputs(&chain, &node).len(), 1, "a confirming fee is not paid twice");

    *chain.0.tip.lock().unwrap() = start;
    chain.mine();
    node.publish_admission().unwrap();
    assert!(queued_admission(&node).is_none(), "the old amount does not meet the raise in force at its height");
    assert_eq!(fee_outputs(&chain, &node), vec![FEE_SATS, RAISED]);
}

#[test]
fn a_reorg_that_moves_the_fee_past_the_start_is_paid_again() {
    let start = 5_000;
    let document = raised(11, start);
    let dir = TempDir::new();
    let chain = MockChain::new();
    *chain.0.tip.lock().unwrap() = start - 30;
    let node = seller(&dir, &chain, &document);
    harness::fund_node_wallet(&node, &chain, 200_000);
    harness::fund_node_wallet(&node, &chain, 200_000);
    pay_from_outside(&chain, fee_script(node.own_store.as_bytes()), FEE_SATS);
    confirm_at(&chain, start - 1);
    node.publish_admission().unwrap();
    let admission = queued_admission(&node).expect("listed under the old fee");
    assert_eq!(admission.height, start - 1);

    chain.reorg();
    chain.mine();
    assert!(!fee_confirmed(&chain, NetworkMode::Testnet4, &document, &admission), "the height no longer matches");
    node.publish_admission().unwrap();
    assert_eq!(fee_outputs(&chain, &node), vec![FEE_SATS, RAISED], "the moved payment is short under the raise");
}
