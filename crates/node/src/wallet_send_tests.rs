//! Sending from the node wallet to an address the user names, against a mocked chain.

use serde_json::{json, Value};

use crate::harness::{
    checkout, fund_node_wallet, open_node, request, ticket, unreachable_escrow, MockChain, TempDir,
    PASSPHRASE,
};
use crate::network::{address_for, NetworkMode};
use crate::Node;

/// A testnet address no node in these tests owns (BIP 173).
const ELSEWHERE: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
const MAINNET: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";

fn funded(network: NetworkMode, sats: u64) -> (TempDir, MockChain, Node) {
    let dir = TempDir::new();
    let chain = MockChain::new();
    *chain.0.fee_rate.lock().unwrap() = 2.0;
    let node = open_node(&dir.0, network, &chain, 1, unreachable_escrow());
    fund_node_wallet(&node, &chain, sats);
    (dir, chain, node)
}

fn quote(node: &Node, body: Value) -> (String, Value) {
    request(node, "POST", "/wallet/send/quote", &body.to_string())
}

fn send(node: &Node, body: Value) -> (String, Value) {
    request(node, "POST", "/wallet/send", &body.to_string())
}

fn confirm(quote: &Value) -> Value {
    json!({ "quote": quote["quote"], "passphrase": PASSPHRASE, "confirm": true })
}

fn spendable(node: &Node) -> u64 {
    let (status, wallet) = request(node, "GET", "/wallet", "");
    assert!(status.contains("200"), "{status}");
    wallet["balance"]["spendableSats"].as_u64().unwrap()
}

fn paid(tx: &bitcoin::Transaction, address: &str) -> Vec<u64> {
    let script = address_for(address, NetworkMode::Testnet4)
        .unwrap()
        .script_pubkey();
    tx.output
        .iter()
        .filter(|out| out.script_pubkey == script)
        .map(|out| out.value.to_sat())
        .collect()
}

#[test]
fn a_send_pays_exactly_the_quoted_amount_to_the_quoted_address() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (status, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 25_000 }));
    assert!(status.contains("200"), "{status} {quoted}");
    assert_eq!(quoted["amountSats"], 25_000);
    assert_eq!(quoted["max"], false);
    assert_eq!(quoted["feeRateSatVb"], 2.0);
    let fee = quoted["feeSats"].as_u64().unwrap();
    assert!(
        fee > 0 && quoted["feeCapSats"].as_u64().unwrap() >= fee,
        "{quoted}"
    );
    assert_eq!(quoted["totalSats"], 25_000 + fee);
    assert!(chain.broadcasts().is_empty(), "a quote sends nothing");

    let (status, sent) = send(&node, confirm(&quoted));
    assert!(status.contains("200"), "{status} {sent}");
    let txs = chain.broadcasts();
    assert_eq!(txs.len(), 1);
    assert_eq!(sent["txid"], txs[0].compute_txid().to_string());
    assert_eq!(paid(&txs[0], ELSEWHERE), vec![25_000]);
    let recipient = address_for(ELSEWHERE, NetworkMode::Testnet4)
        .unwrap()
        .script_pubkey();
    let wallet = node.buyer_wallet();
    for out in txs[0]
        .output
        .iter()
        .filter(|out| out.script_pubkey != recipient)
    {
        assert!(
            wallet.wallet.is_mine(out.script_pubkey.clone()),
            "every other output is change"
        );
    }
    drop(wallet);
    assert_eq!(
        spendable(&node),
        100_000 - 25_000 - sent["feeSats"].as_u64().unwrap()
    );
}

#[test]
fn send_max_empties_the_spendable_balance_into_one_output() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 60_000);
    fund_node_wallet(&node, &chain, 40_000);
    let (status, quoted) = quote(&node, json!({ "address": ELSEWHERE, "max": true }));
    assert!(status.contains("200"), "{status} {quoted}");
    let fee = quoted["feeSats"].as_u64().unwrap();
    assert_eq!(quoted["max"], true);
    assert_eq!(quoted["amountSats"], 100_000 - fee);
    let (status, sent) = send(&node, confirm(&quoted));
    assert!(status.contains("200"), "{status} {sent}");
    let tx = &chain.broadcasts()[0];
    assert_eq!(tx.output.len(), 1, "no change");
    assert_eq!(tx.input.len(), 2, "both coins");
    assert_eq!(paid(tx, ELSEWHERE), vec![100_000 - fee]);
    assert_eq!(spendable(&node), 0);
}

#[test]
fn bad_destinations_and_amounts_are_refused_before_anything_is_built() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    for (body, needle) in [
        (
            json!({ "address": MAINNET, "amountSats": 10_000 }),
            "not a testnet4 address",
        ),
        (
            json!({ "address": "not an address", "amountSats": 10_000 }),
            "not a bitcoin address",
        ),
        (
            json!({ "address": "", "amountSats": 10_000 }),
            "Enter the address",
        ),
        (json!({ "address": ELSEWHERE }), "Enter an amount"),
        (
            json!({ "address": ELSEWHERE, "amountSats": 0 }),
            "above zero",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": -5 }),
            "above zero",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": 1.5 }),
            "above zero",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": "1000" }),
            "above zero",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": 100 }),
            "dust limit",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": 1000, "max": true }),
            "not both",
        ),
        (
            json!({ "address": ELSEWHERE, "amountSats": 1000, "fee": 1 }),
            "not part of a send",
        ),
    ] {
        let (status, value) = quote(&node, body.clone());
        assert!(status.contains("400"), "{body}: {status} {value}");
        assert!(
            value["error"].as_str().unwrap().contains(needle),
            "{body}: {value}"
        );
    }
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn a_send_above_the_balance_says_how_much_is_missing() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 20_000);
    let (status, value) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 50_000 }));
    assert!(status.contains("400"), "{status} {value}");
    let error = value["error"].as_str().unwrap();
    assert!(
        error.contains("Not enough funds") && error.contains("can spend 20000 sats"),
        "{error}"
    );
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn every_send_needs_the_passphrase_and_confirm_even_while_unlocked() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (_, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    for body in [
        json!({ "quote": quoted["quote"], "confirm": true }),
        json!({ "quote": quoted["quote"], "passphrase": "", "confirm": true }),
        json!({ "quote": quoted["quote"], "passphrase": PASSPHRASE }),
        json!({ "quote": quoted["quote"], "passphrase": PASSPHRASE, "confirm": false }),
        json!({ "passphrase": PASSPHRASE, "confirm": true }),
        json!({ "quote": quoted["quote"], "passphrase": PASSPHRASE, "confirm": true, "amountSats": 99 }),
    ] {
        let (status, value) = send(&node, body.clone());
        assert!(status.contains("400"), "{body}: {status} {value}");
    }
    let (status, value) = send(
        &node,
        json!({ "quote": quoted["quote"], "passphrase": "wrong horse", "confirm": true }),
    );
    assert!(status.contains("400"), "{status} {value}");
    assert_eq!(value["code"], "wrong_passphrase");
    assert!(chain.broadcasts().is_empty());
    // None of those used the quote up.
    let (status, value) = send(&node, confirm(&quoted));
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!(chain.broadcasts().len(), 1);
}

#[test]
fn a_quote_sends_once_however_often_it_is_submitted() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (_, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    let (first, _) = send(&node, confirm(&quoted));
    assert!(first.contains("200"), "{first}");
    let (second, value) = send(&node, confirm(&quoted));
    assert!(second.contains("409"), "{second} {value}");
    assert_eq!(chain.broadcasts().len(), 1);
    let (made_up, _) = send(
        &node,
        json!({ "quote": "00".repeat(32), "passphrase": PASSPHRASE, "confirm": true }),
    );
    assert!(made_up.contains("409"), "{made_up}");
}

#[test]
fn a_failed_broadcast_uses_the_quote_up_and_says_the_network_is_unreachable() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (_, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    chain
        .0
        .fail_broadcast
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (status, value) = send(&node, confirm(&quoted));
    assert!(status.contains("502"), "{status} {value}");
    let (status, _) = send(&node, confirm(&quoted));
    assert!(
        status.contains("409"),
        "a retry needs a fresh review: {status}"
    );
    assert!(chain.broadcasts().is_empty());
    assert_eq!(spendable(&node), 100_000);
}

#[test]
fn a_locked_wallet_quotes_and_sends_nothing_and_a_lock_drops_open_quotes() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (_, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    node.lock.lock();
    let (status, value) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    assert!(status.contains("423"), "{status} {value}");
    assert_eq!(value["code"], "locked");
    let (status, value) = send(&node, confirm(&quoted));
    assert!(status.contains("423"), "{status} {value}");
    crate::harness::unlock(&node);
    let (status, _) = send(&node, confirm(&quoted));
    assert!(
        status.contains("409"),
        "the lock dropped the quote: {status}"
    );
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn a_foreign_page_cannot_reach_the_send_routes() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let (_, quoted) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    let body = confirm(&quoted).to_string();
    let raw = format!(
        "POST /wallet/send HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nX-Ob-Pair: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        node.pairing_token(),
        body.len(),
    );
    let response = String::from_utf8(crate::http::dispatch(&node, raw.as_bytes())).unwrap();
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    let unpaired = format!(
        "POST /wallet/send HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len(),
    );
    let response = String::from_utf8(crate::http::dispatch(&node, unpaired.as_bytes())).unwrap();
    assert!(response.starts_with("HTTP/1.1 403"), "{response}");
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn a_mainnet_node_sends_only_to_mainnet_addresses() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(
        &dir.0,
        NetworkMode::Mainnet,
        &chain,
        1,
        unreachable_escrow(),
    );
    fund_node_wallet(&node, &chain, 100_000);
    let (status, value) = quote(&node, json!({ "address": ELSEWHERE, "amountSats": 10_000 }));
    assert!(
        status.contains("400") && value["error"].as_str().unwrap().contains("mainnet"),
        "{status} {value}"
    );
    let (status, quoted) = quote(&node, json!({ "address": MAINNET, "amountSats": 10_000 }));
    assert!(status.contains("200"), "{status} {quoted}");
    assert_eq!(quoted["network"], "mainnet");
    let (status, _) = send(&node, confirm(&quoted));
    assert!(status.contains("200"), "{status}");
}

#[test]
fn change_from_an_order_payment_is_held_back_unless_the_user_links_it() {
    let (_dir, chain, node) = funded(NetworkMode::Testnet4, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    node.pay_order(id, &json!({ "ticket": ticket(&node, id).unwrap() }))
        .unwrap();
    let change = spendable(&node);
    assert!(change > 0);

    let (status, value) = quote(&node, json!({ "address": ELSEWHERE, "max": true }));
    assert!(status.contains("400"), "{status} {value}");
    assert!(
        value["error"].as_str().unwrap().contains("held back"),
        "{value}"
    );
    let (status, quoted) = quote(
        &node,
        json!({ "address": ELSEWHERE, "max": true, "linkCoins": true }),
    );
    assert!(status.contains("200"), "{status} {quoted}");
    assert_eq!(quoted["heldBackSats"], 0);
    assert_eq!(
        quoted["amountSats"].as_u64().unwrap() + quoted["feeSats"].as_u64().unwrap(),
        change
    );
    let (status, _) = send(&node, confirm(&quoted));
    assert!(status.contains("200"), "{status}");
    assert_eq!(chain.broadcasts().len(), 2);
}

#[test]
fn one_node_wallet_sends_to_another() {
    let chain = MockChain::new();
    let (alice_dir, bob_dir) = (TempDir::new(), TempDir::new());
    let alice = open_node(
        &alice_dir.0,
        NetworkMode::Testnet4,
        &chain,
        1,
        unreachable_escrow(),
    );
    let bob = open_node(
        &bob_dir.0,
        NetworkMode::Testnet4,
        &chain,
        1,
        unreachable_escrow(),
    );
    fund_node_wallet(&alice, &chain, 80_000);
    let (_, to) = request(&bob, "POST", "/wallet/address", "");
    let to = to["address"].as_str().unwrap().to_string();

    let (status, quoted) = quote(&alice, json!({ "address": to, "amountSats": 30_000 }));
    assert!(status.contains("200"), "{status} {quoted}");
    let (status, sent) = send(&alice, confirm(&quoted));
    assert!(status.contains("200"), "{status} {sent}");
    // The mock chain hands the broadcast to every wallet sync, confirmed.
    let tx = chain.broadcasts()[0].clone();
    chain.0.wallet_txs.lock().unwrap().push((tx, true));
    bob.sync_buyer_wallet().unwrap();
    assert_eq!(spendable(&bob), 30_000);
    let (_, wallet) = request(&bob, "GET", "/wallet", "");
    assert_eq!(wallet["activity"][0]["txid"], sent["txid"]);
    assert_eq!(wallet["activity"][0]["direction"], "in");
}
