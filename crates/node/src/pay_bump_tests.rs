//! Raising the fee of an unconfirmed direct payment, against a mocked chain.

use serde_json::{json, Value};

use crate::harness::{
    checkout, fund_node_wallet, open_node, order, request, ticket, unreachable_escrow, MockChain, TempDir, PASSPHRASE,
};
use crate::network::NetworkMode;
use crate::Node;

fn paid_order(network: NetworkMode) -> (TempDir, MockChain, Node, String, String) {
    let dir = TempDir::new();
    let chain = MockChain::new();
    *chain.0.fee_rate.lock().unwrap() = 1.0;
    let node = open_node(&dir.0, network, &chain, 1, unreachable_escrow());
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    let mut pay = json!({"ticket": ticket(&node, &id).unwrap()});
    if network == NetworkMode::Mainnet {
        pay["confirm"] = json!(true);
    }
    let paid = node.pay_order(&id, &pay).unwrap();
    (dir, chain, node, id, paid["txid"].as_str().unwrap().to_string())
}

fn bump(node: &Node, id: &str, body: Value) -> (String, Value) {
    request(node, "POST", &format!("/orders/{id}/pay/bump"), &body.to_string())
}

fn fast(max_fee: u64) -> Value {
    json!({"feeRateSatVb": 5.0, "maxFeeSats": max_fee, "passphrase": PASSPHRASE})
}

#[test]
fn a_bump_replaces_the_payment_with_the_same_one_at_a_higher_fee() {
    let (_dir, chain, node, id, first) = paid_order(NetworkMode::Testnet4);
    let address = order(&node, &id)["payment"]["address"].as_str().unwrap().to_string();
    let original = chain.broadcasts()[0].clone();

    let (status, bumped) = bump(&node, &id, fast(10_000));
    assert!(status.contains("200"), "{status} {bumped}");
    assert_eq!(bumped["replacedTxid"], first);
    let second = bumped["txid"].as_str().unwrap();
    assert_ne!(second, first);
    assert_eq!(order(&node, &id)["payTxid"], second);

    let replacement = chain.broadcasts()[1].clone();
    assert_eq!(replacement.compute_txid().to_string(), second);
    assert!(
        original.input.iter().all(|old| replacement.input.iter().any(|new| new.previous_output == old.previous_output)),
        "the replacement spends the payment's inputs"
    );
    let to_seller = |tx: &bitcoin::Transaction| {
        let script = crate::network::address_for(&address, NetworkMode::Testnet4).unwrap().script_pubkey();
        tx.output.iter().filter(|out| out.script_pubkey == script).map(|out| out.value).collect::<Vec<_>>()
    };
    assert_eq!(to_seller(&replacement), to_seller(&original), "the seller gets the same amount");
    let fee = bumped["feeSats"].as_u64().unwrap();
    assert!(fee as f64 >= 5.0 * replacement.vsize() as f64, "fee {fee} for {} vB", replacement.vsize());

    node.watch_once().unwrap();
    let seen = order(&node, &id);
    assert_eq!(seen["status"], "Payment seen");
    assert_eq!(seen["txids"], json!([second]), "the replaced payment no longer counts");
    chain.mine();
    node.watch_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Paid");

    let (status, late) = bump(&node, &id, fast(10_000));
    assert!(status.contains("409"), "a confirmed payment is not bumped: {status} {late}");
    assert_eq!(chain.broadcasts().len(), 2);
}

#[test]
fn a_bump_needs_the_passphrase_a_fee_cap_and_a_payment() {
    let (_dir, chain, node, id, _) = paid_order(NetworkMode::Testnet4);

    let wrong = json!({"feeRateSatVb": 5.0, "maxFeeSats": 10_000, "passphrase": "not it"});
    let (status, _) = bump(&node, &id, wrong);
    assert!(!status.contains("200"), "{status}");
    let (status, capped) = bump(&node, &id, fast(10));
    assert!(status.contains("409"), "{status} {capped}");
    assert!(capped["error"].as_str().unwrap().contains("Nothing was sent"));
    let (status, _) = bump(&node, &id, json!({"feeRateSatVb": 900.0, "maxFeeSats": 10_000, "passphrase": PASSPHRASE}));
    assert!(status.contains("400"), "{status}");
    let (status, _) = bump(&node, &id, json!({"feeRateSatVb": 5.0, "maxFeeSats": 10_000, "passphrase": PASSPHRASE, "to": "x"}));
    assert!(status.contains("400"), "{status}");

    let unpaid = checkout(&node, &[("Letterpress cards", "Hale Paper", "direct")]);
    let unpaid = unpaid[0]["orderId"].as_str().unwrap();
    let (status, none) = bump(&node, unpaid, fast(10_000));
    assert!(status.contains("409"), "{status} {none}");
    let (status, _) = bump(&node, "no-such-order", fast(10_000));
    assert!(status.contains("404"), "{status}");
    assert_eq!(chain.broadcasts().len(), 1, "nothing was replaced");
}

#[test]
fn mainnet_bumps_need_the_confirm_flag() {
    let (_dir, chain, node, id, _) = paid_order(NetworkMode::Mainnet);
    let (status, refused) = bump(&node, &id, fast(10_000));
    assert!(status.contains("400"), "{status}");
    assert!(refused["error"].as_str().unwrap().contains("confirm"));
    let mut confirmed = fast(10_000);
    confirmed["confirm"] = json!(true);
    let (status, bumped) = bump(&node, &id, confirmed);
    assert!(status.contains("200"), "{status} {bumped}");
    assert_eq!(chain.broadcasts().len(), 2);
}
