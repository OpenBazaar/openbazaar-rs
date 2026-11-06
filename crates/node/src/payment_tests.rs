//! Payments against a mocked chain source. Nothing here touches the internet.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use bitcoin::Address;
use serde_json::{json, Value};

use crate::harness::{
    checkout, fund_node_wallet, hash_of, open_node, order, pay, request, ship_to, unreachable_escrow, MockChain, TempDir,
};
use crate::http::dispatch;
use crate::network::{address_for, NetworkMode, ServeOptions};
use crate::{load_catalog, EscrowConfig, Node, NodeConfig, ESCROW_SPEND_FEE_SATS};

fn open(dir: &Path, network: NetworkMode, chain: &MockChain, confirmations: u32) -> Node {
    open_node(dir, network, chain, confirmations, unreachable_escrow())
}

fn testnet() -> (TempDir, MockChain, Node) {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open(&dir.0, NetworkMode::Testnet4, &chain, 1);
    (dir, chain, node)
}

#[test]
fn direct_order_moves_from_awaiting_to_seen_to_paid_and_persists() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open(&dir.0, NetworkMode::Testnet4, &chain, 2);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap().to_string();
    let address = placed[0]["payment"]["address"].as_str().unwrap().to_string();
    let total = placed[0]["totalSats"].as_u64().unwrap();
    assert_eq!(order(&node, &id)["status"], "Awaiting payment");

    let txid = chain.pay_external(&address, total);
    assert_eq!(node.watch_once().unwrap(), 1);
    let seen = order(&node, &id);
    assert_eq!(seen["status"], "Payment seen");
    assert_eq!(seen["receivedSats"], total);
    assert_eq!(seen["confirmations"], 0);
    assert_eq!(seen["txids"][0], txid.to_string());

    chain.mine();
    node.watch_once().unwrap();
    let one = order(&node, &id);
    assert_eq!(one["status"], "Payment seen", "two confirmations are required");
    assert_eq!(one["confirmations"], 1);

    chain.mine();
    node.watch_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Paid");
    assert_eq!(node.watch_once().unwrap(), 0, "a paid order is not queried again");

    let stored = fs::read(dir.0.join("orders.obv")).unwrap();
    assert!(!String::from_utf8_lossy(&stored).contains("Paid"));
    drop(node);
    let reopened = open(&dir.0, NetworkMode::Testnet4, &chain, 2);
    let (_, list) = request(&reopened, "GET", "/orders", "");
    assert_eq!(list["orders"][0]["status"], "Paid");
    assert_eq!(list["orders"][0]["confirmations"], 2);
}

#[test]
fn underpayment_stays_awaiting_and_shows_what_arrived() {
    let (_dir, chain, node) = testnet();
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let address = placed[0]["payment"]["address"].as_str().unwrap();
    let total = placed[0]["totalSats"].as_u64().unwrap();

    chain.pay_external(address, total - 1);
    chain.mine();
    node.watch_once().unwrap();
    let short = order(&node, id);
    assert_eq!(short["status"], "Awaiting payment");
    assert_eq!(short["receivedSats"], total - 1);

    chain.pay_external(address, 1);
    node.watch_once().unwrap();
    assert_eq!(order(&node, id)["status"], "Payment seen");
    chain.mine();
    node.watch_once().unwrap();
    let paid = order(&node, id);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(paid["receivedSats"], total);
    assert_eq!(paid["txids"].as_array().unwrap().len(), 2);
}

#[test]
fn no_receive_address_is_reused_across_orders_or_restarts() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let mut seen = HashSet::new();
    for _ in 0..2 {
        let node = open(&dir.0, NetworkMode::Testnet4, &chain, 1);
        for _ in 0..3 {
            let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
            let address = placed[0]["payment"]["address"].as_str().unwrap().to_string();
            assert!(seen.insert(address.clone()), "{address} was handed out twice");
            let escrow = checkout(&node, &[("Paperback novel", "Marlow Goods", "escrow")]);
            let value = escrow[0]["escrow"]["buyerPayout"].as_str().unwrap().to_string();
            assert!(seen.insert(value.clone()), "buyerPayout {value} was handed out twice");
        }
    }
    assert_eq!(seen.len(), 2 * 3 * 2);
}

#[test]
fn multi_seller_checkout_gets_separate_wallets_and_separate_transactions() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 200_000);
    let placed = checkout(
        &node,
        &[
            ("Paperback novel", "Marlow Goods", "direct"),
            ("Letterpress cards", "Hale Paper", "direct"),
            ("Whole bean coffee, 12 oz", "North Kitchen", "direct"),
        ],
    );
    assert_eq!(placed.len(), 3);
    let addresses: Vec<Address> = placed
        .iter()
        .map(|order| address_for(order["payment"]["address"].as_str().unwrap(), NetworkMode::Testnet4).unwrap())
        .collect();
    assert_eq!(addresses.iter().collect::<HashSet<_>>().len(), 3);
    for (order, address) in placed.iter().zip(&addresses) {
        let seller = order["seller"].as_str().unwrap();
        for other in ["Marlow Goods", "Hale Paper", "North Kitchen"] {
            let mine = node.store_wallet(other).unwrap().wallet.is_mine(address.script_pubkey());
            assert_eq!(mine, other == seller, "{seller}'s address in {other}'s wallet");
        }
        assert!(!node.buyer_wallet().wallet.is_mine(address.script_pubkey()));
    }
    let codes: HashSet<_> = placed.iter().map(|order| order["payment"]["silentPaymentCode"].clone()).collect();
    assert_eq!(codes.len(), 3, "each seller has its own silent-payment code");

    for order in &placed {
        let id = order["orderId"].as_str().unwrap();
        let (status, paid) = pay(&node, id, "{}");
        assert!(status.contains("202"), "{status} {paid}");
        assert!(paid["payTxid"].is_string(), "{paid}");
    }
    let sent = chain.broadcasts();
    assert_eq!(sent.len(), 3);
    for (tx, (order, address)) in sent.iter().zip(placed.iter().zip(&addresses)) {
        let to_orders: Vec<_> = tx
            .output
            .iter()
            .filter(|out| addresses.iter().any(|a| a.script_pubkey() == out.script_pubkey))
            .collect();
        assert_eq!(to_orders.len(), 1, "one seller per transaction");
        assert_eq!(to_orders[0].script_pubkey, address.script_pubkey());
        assert_eq!(to_orders[0].value.to_sat(), order["totalSats"].as_u64().unwrap());
        let wallet = node.buyer_wallet();
        assert!(tx.output.iter().all(|out| out.script_pubkey == address.script_pubkey()
            || wallet.wallet.is_mine(out.script_pubkey.clone())));
    }
    chain.mine();
    node.watch_once().unwrap();
    for order in &placed {
        assert_eq!(self::order(&node, order["orderId"].as_str().unwrap())["status"], "Paid");
    }
}

#[test]
fn paying_twice_is_refused_and_the_fee_rate_has_a_floor() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    *chain.0.fee_rate.lock().unwrap() = 0.1;
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let paid = node.pay_order(id, &json!({})).unwrap();
    assert_eq!(paid["feeRateSatVb"], 1.0);
    assert_eq!(paid["payjoin"], false);
    assert_eq!(order(&node, id)["payTxid"], paid["txid"]);
    let tx = &chain.broadcasts()[0];
    let fee = paid["feeSats"].as_u64().unwrap();
    assert!(fee as f64 >= tx.vsize() as f64, "fee {fee} for {} vB", tx.vsize());

    let (status, again) = request(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("409"), "{status} {again}");
    assert_eq!(node.run_pay_jobs(), 0);
    assert_eq!(chain.broadcasts().len(), 1);

    *chain.0.fee_rate.lock().unwrap() = 5_000.0;
    let other = checkout(&node, &[("Letterpress cards", "Hale Paper", "direct")]);
    let other_id = other[0]["orderId"].as_str().unwrap();
    let (_, after) = pay(&node, other_id, "{}");
    assert!(after["payError"].as_str().unwrap().contains("above 500"), "{after}");
    assert!(after.get("payTxid").is_none());
    assert_eq!(chain.broadcasts().len(), 1);
}

#[test]
fn pay_answers_at_once_and_the_worker_sends_it_once() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let (status, queued) = request(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("202"), "{status} {queued}");
    assert_eq!(queued["status"], "Awaiting payment");
    assert_eq!(queued["paying"], true);
    assert!(chain.broadcasts().is_empty(), "nothing is sent on the request thread");
    let (status, again) = request(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("409"), "{status}");
    assert!(again["error"].as_str().unwrap().contains("already being sent"));

    assert_eq!(node.run_pay_jobs(), 1);
    assert_eq!(chain.broadcasts().len(), 1);
    let after = order(&node, id);
    assert_eq!(after["paying"], false);
    assert_eq!(after["payTxid"], json!(chain.broadcasts()[0].compute_txid().to_string()));
    assert!(after.get("payError").is_none());
}

#[test]
fn unconfirmed_coins_from_others_are_not_spent_but_own_change_is() {
    let (_dir, chain, node) = testnet();
    let address = node.buyer_wallet().fresh_address().unwrap();
    chain.send_to_wallet(&address, 100_000, false);
    node.watch_once().unwrap();
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    assert_eq!(wallet["balance"]["pendingSats"], 100_000);
    assert_eq!(wallet["balance"]["spendableSats"], 0);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let (_, after) = pay(&node, id, "{}");
    assert!(after["payError"].as_str().unwrap().contains("cannot pay"), "{after}");
    assert!(chain.broadcasts().is_empty());

    chain.0.wallet_txs.lock().unwrap()[0].1 = true;
    let (_, paid) = pay(&node, id, "{}");
    assert!(paid["payTxid"].is_string(), "{paid}");
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    let change = wallet["balance"]["spendableSats"].as_u64().unwrap();
    assert!(change > 0 && wallet["balance"]["confirmedSats"] == 0, "{wallet}");

    let other = checkout(&node, &[("Letterpress cards", "Hale Paper", "direct")]);
    let (_, second) = pay(&node, other[0]["orderId"].as_str().unwrap(), "{}");
    assert!(second["payTxid"].is_string(), "unconfirmed change is spendable: {second}");
    assert_eq!(chain.broadcasts()[1].input[0].previous_output.txid, chain.broadcasts()[0].compute_txid());
}

#[test]
fn an_empty_wallet_cannot_pay_and_nothing_is_sent() {
    let (_dir, chain, node) = testnet();
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let (_, after) = pay(&node, id, "{}");
    assert!(after["payError"].as_str().unwrap().contains("cannot pay"), "{after}");
    assert!(chain.broadcasts().is_empty());
    assert_eq!(after["status"], "Awaiting payment");
}

#[test]
fn mainnet_pay_needs_the_confirm_flag() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open(&dir.0, NetworkMode::Mainnet, &chain, 3);
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    assert!(placed[0]["payment"]["address"].as_str().unwrap().starts_with("bc1p"));
    assert!(placed[0]["payment"]["silentPaymentCode"].as_str().unwrap().starts_with("sp1"));
    assert_eq!(placed[0]["network"], "mainnet");

    for body in ["", "{}", r#"{"confirm": false}"#] {
        let (status, value) = request(&node, "POST", &format!("/orders/{id}/pay"), body);
        assert!(status.contains("400"), "{body}: {status}");
        assert!(value["error"].as_str().unwrap().contains("confirm"));
    }
    let (status, _) = request(&node, "POST", &format!("/orders/{id}/pay"), r#"{"confirm": "yes"}"#);
    assert!(status.contains("400"));
    assert!(chain.broadcasts().is_empty());

    let (status, paid) = pay(&node, id, r#"{"confirm": true}"#);
    assert!(status.contains("202"), "{status} {paid}");
    assert_eq!(paid["network"], "mainnet");
    assert!(paid["payTxid"].is_string(), "{paid}");
    assert_eq!(chain.broadcasts().len(), 1);
}

#[test]
fn mainnet_refuses_escrow_and_testnet4_labels_it_an_unsafe_demo() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let main = open(&dir.0, NetworkMode::Mainnet, &chain, 3);
    let body = json!({"shipTo": ship_to(), "packages": [
        {"settlement": "escrow", "lines": [{"listingHash": hash_of(&main, "Paperback novel", "Marlow Goods"), "quantity": 1}]}
    ]});
    let (status, value) = request(&main, "POST", "/checkout", &body.to_string());
    assert!(status.contains("400"), "{status} {value}");
    assert!(value["error"].as_str().unwrap().contains("not available on mainnet"));
    assert!(main.mailbox.list().unwrap().is_empty(), "nothing is sealed for a refused escrow");
    let single = json!({"listingHash": hash_of(&main, "Paperback novel", "Marlow Goods"), "quantity": 1, "settlement": "escrow", "shipTo": ship_to()});
    let (status, _) = request(&main, "POST", "/orders", &single.to_string());
    assert!(status.contains("400"));
    let (status, value) = request(&main, "POST", "/escrow/mail", "sealed");
    assert!(status.contains("400"), "{status}");
    assert!(value["error"].as_str().unwrap().contains("not available on mainnet"));
    assert!(main.escrow.inbox.list().unwrap().is_empty());

    let (_dir, _chain, test) = testnet();
    let placed = checkout(&test, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let terms = &placed[0]["escrow"];
    assert_eq!(terms["demo"], true);
    assert_eq!(terms["unsafe"], true);
    assert!(terms["warning"].as_str().unwrap().contains("testnet4 coins only"));
    assert_eq!(terms["fundingSats"], placed[0]["totalSats"].as_u64().unwrap() + ESCROW_SPEND_FEE_SATS);
    assert!(terms["error"].as_str().unwrap().contains("127.0.0.1:9"), "the unreachable vendor is reported: {terms}");
    assert_eq!(placed[0]["status"], crate::AWAITING_TERMS);
    let (status, value) = request(&test, "POST", &format!("/orders/{}/pay", placed[0]["orderId"].as_str().unwrap()), "{}");
    assert!(status.contains("409"), "no terms, nothing to fund: {status} {value}");
}

#[test]
fn escrow_needs_a_vendor_on_another_node() {
    let chain = MockChain::new();
    let alone = TempDir::new();
    let node = open_node(&alone.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
    let body = json!({"shipTo": ship_to(), "packages": [
        {"settlement": "escrow", "lines": [{"listingHash": hash_of(&node, "Paperback novel", "Marlow Goods"), "quantity": 1}]}
    ]});
    let (status, value) = request(&node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("400"), "{status} {value}");
    assert!(node.mailbox.list().unwrap().is_empty(), "nothing is sealed for a refused escrow");

    let selfish = TempDir::new();
    let own = "http://127.0.0.1:4242".to_string();
    let node = open_node(&selfish.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig {
        mailbox_url: Some(own.clone()),
        vendors: [("Marlow Goods".to_string(), own)].into_iter().collect(),
        ..EscrowConfig::default()
    });
    let (status, value) = request(&node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("400"), "a node is never its own vendor: {status} {value}");
}

/// Every string in a testnet4 response that parses as an address is a testnet4 address.
#[test]
fn testnet4_never_shows_a_mainnet_address() {
    fn walk(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                let text = text.strip_prefix("bitcoin:").unwrap_or(text);
                let text = text.split('?').next().unwrap();
                if text.parse::<Address<_>>().is_ok() {
                    found.push(text.to_string());
                }
                assert!(!text.starts_with("sp1"), "mainnet silent-payment code {text}");
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, found)),
            Value::Object(map) => map.values().for_each(|item| walk(item, found)),
            _ => {}
        }
    }
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(
        &node,
        &[("Paperback novel", "Marlow Goods", "escrow"), ("Letterpress cards", "Hale Paper", "direct")],
    );
    pay(&node, placed[1]["orderId"].as_str().unwrap(), "{}");
    let mut found = Vec::new();
    for value in [
        json!(placed),
        request(&node, "GET", "/orders", "").1,
        request(&node, "GET", "/wallet", "").1,
        request(&node, "POST", "/wallet/address", "").1,
    ] {
        walk(&value, &mut found);
    }
    assert!(found.len() >= 6, "{found:?}");
    for address in found {
        assert!(address_for(&address, NetworkMode::Testnet4).is_ok(), "{address}");
    }
    for offer in [placed[0].clone(), placed[1].clone()].iter().filter_map(|order| order["payment"]["lightning"].as_str().map(str::to_string)) {
        assert!(crate::network::offer_for(&offer, NetworkMode::Testnet4).is_ok());
    }
}

#[test]
fn wrong_network_strings_are_refused() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    node.orders()
        .update(id, |order| {
            order.payment.as_mut().unwrap()["address"] = json!("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");
        })
        .unwrap();
    let (status, value) = request(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("400"), "{status}");
    assert!(value["error"].as_str().unwrap().contains("testnet4"));
    assert!(chain.broadcasts().is_empty());

    let other = TempDir::new();
    let mainnet_catalog = load_catalog(NetworkMode::Mainnet).unwrap();
    let refused = Node::open_with(&other.0, mainnet_catalog, NodeConfig::offline(NetworkMode::Testnet4));
    assert!(refused.is_err(), "a mainnet catalog on a testnet4 node");
}

#[test]
fn each_network_has_its_own_data_dir_and_never_shows_the_other() {
    let root = TempDir::new();
    let chain = MockChain::new();
    let args = |extra: &[&str]| {
        let mut list = vec!["--data-dir".to_string(), root.0.display().to_string()];
        list.extend(extra.iter().map(|text| text.to_string()));
        ServeOptions::parse(&list).unwrap()
    };
    let test_dir = args(&[]).data_dir();
    let main_dir = args(&["--network", "mainnet"]).data_dir();
    assert_eq!(test_dir, root.0.join("testnet4"));
    assert_eq!(main_dir, root.0.join("mainnet"));

    let test = open(&test_dir, NetworkMode::Testnet4, &chain, 1);
    let main = open(&main_dir, NetworkMode::Mainnet, &chain, 3);
    checkout(&test, &[("Paperback novel", "Marlow Goods", "direct")]);
    let (_, main_orders) = request(&main, "GET", "/orders", "");
    assert_eq!(main_orders["orders"].as_array().unwrap().len(), 0);
    assert_ne!(test.pairing_token(), main.pairing_token());
    for dir in [&test_dir, &main_dir] {
        for file in ["vault.key", "pairing.token", "wallets/node/seed.obv", "wallets/store-marlow-goods/seed.obv"] {
            assert!(dir.join(file).exists(), "{}", dir.join(file).display());
        }
    }
    assert_ne!(
        fs::read(test_dir.join("vault.key")).unwrap(),
        fs::read(main_dir.join("vault.key")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in ["wallets/node/seed.obv", "wallets/node/state.obv"] {
            let mode = fs::metadata(test_dir.join(file)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    let (_, test_wallet) = request(&test, "GET", "/wallet", "");
    let (_, main_wallet) = request(&main, "GET", "/wallet", "");
    assert!(test_wallet["receiveAddress"].as_str().unwrap().starts_with("tb1p"));
    assert!(main_wallet["receiveAddress"].as_str().unwrap().starts_with("bc1p"));

    // A record stamped with another network, even in this file, is not shown or paid.
    let id = {
        let (_, list) = request(&test, "GET", "/orders", "");
        list["orders"][0]["id"].as_str().unwrap().to_string()
    };
    test.orders().update(&id, |order| order.network = "mainnet".to_string()).unwrap();
    assert!(test.orders().get(&id).is_none());
    let (_, list) = request(&test, "GET", "/orders", "");
    assert_eq!(list["orders"].as_array().unwrap().len(), 0);
    let (status, _) = request(&test, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("404"));
}

#[test]
fn network_and_wallet_endpoints() {
    let (_dir, chain, node) = testnet();
    let raw = b"GET /network HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\n\r\n";
    let text = String::from_utf8(dispatch(&node, raw)).unwrap();
    let body: Value = serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["network"], "testnet4");
    assert_eq!(body["confirmations"], 1);

    let raw = b"GET /wallet HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\n\r\n";
    let text = String::from_utf8(dispatch(&node, raw)).unwrap();
    assert!(text.starts_with("HTTP/1.1 403"), "the wallet needs the pairing token");

    let (_, first) = request(&node, "GET", "/wallet", "");
    let (_, again) = request(&node, "GET", "/wallet", "");
    assert_eq!(first["receiveAddress"], again["receiveAddress"], "unused address is stable");
    assert_eq!(first["balance"]["totalSats"], 0);
    let (_, fresh) = request(&node, "POST", "/wallet/address", "");
    let (_, fresher) = request(&node, "POST", "/wallet/address", "");
    assert_ne!(fresh["address"], fresher["address"]);
    assert_ne!(fresh["address"], first["receiveAddress"]);

    let receive = address_for(first["receiveAddress"].as_str().unwrap(), NetworkMode::Testnet4).unwrap();
    chain.fund_wallet(&receive, 25_000);
    node.watch_once().unwrap();
    let (_, funded) = request(&node, "GET", "/wallet", "");
    assert_eq!(funded["balance"]["totalSats"], 25_000);
    assert_ne!(funded["receiveAddress"], first["receiveAddress"], "a used address is not shown again");
}

/// Live testnet4 run. Opt in with:
///
/// `OB_LIVE_DIR=/tmp/ob-live cargo test -p ob-node --offline live_testnet4 -- --ignored --nocapture`
///
/// It prints the node wallet's receive address. Fund that address from a
/// testnet4 faucet, run the test again with the same `OB_LIVE_DIR`, and it
/// pays one order from the node wallet and waits until the order is Paid.
#[test]
#[ignore]
fn live_testnet4() {
    use std::time::{Duration, Instant};
    let dir = PathBuf::from(std::env::var("OB_LIVE_DIR").unwrap_or_else(|_| "/tmp/ob-live-testnet4".to_string()));
    let urls: Vec<String> = match std::env::var("OB_ESPLORA") {
        Ok(list) => list.split(',').map(|url| url.trim().to_string()).collect(),
        Err(_) => NetworkMode::Testnet4.default_esplora().iter().map(|url| url.to_string()).collect(),
    };
    let chain = crate::EsploraChain::new(&urls, NetworkMode::Testnet4.bitcoin()).unwrap();
    let node = Node::open_with(
        &dir,
        load_catalog(NetworkMode::Testnet4).unwrap(),
        NodeConfig {
            network: NetworkMode::Testnet4,
            confirmations: 1,
            public_server: true,
            chain: Box::new(chain),
            escrow: EscrowConfig::default(),
        },
    )
    .unwrap();
    node.sync_buyer_wallet().unwrap();
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    println!("node wallet on testnet4: {wallet:#}");
    let spendable = wallet["balance"]["spendableSats"].as_u64().unwrap();
    if spendable < 20_000 {
        println!(
            "fund {} with at least 20000 testnet4 sats and run again once it confirms",
            wallet["receiveAddress"]
        );
        return;
    }
    let placed = checkout(&node, &[("Graphite pencils", "Hale Paper", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    println!("order {id} pays {}", placed[0]["payment"]["bip21"]);
    let (status, paid) = pay(&node, id, "{}");
    assert!(paid["payTxid"].is_string(), "{status} {paid}");
    println!("broadcast {}", paid["payTxid"]);
    let start = Instant::now();
    loop {
        node.watch_once().unwrap();
        let now = order(&node, id);
        println!("{} after {:?}: {} sats, {} confirmation(s)", now["status"], start.elapsed(), now["receivedSats"], now["confirmations"]);
        if now["status"] == "Paid" {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3 * 3600), "not confirmed within three hours");
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// Send live testnet4 coins from the node wallet in `OB_LIVE_DIR` to another
/// address, for example to fund the desktop app's node wallet. Without
/// `OB_SEND_TO` it only syncs and prints the wallet, so run it once that way
/// first.
///
/// `OB_LIVE_DIR=/tmp/ob-live-testnet4 OB_SEND_TO=tb1... OB_SEND_SATS=12000 cargo test -p ob-node live_testnet4_send -- --ignored --nocapture`
///
/// It broadcasts at once. The transaction has one recipient plus change and
/// uses the node's payment fee rate (the chain estimate, floored at 1 sat/vB).
/// The address must be a testnet4 address.
#[test]
#[ignore]
fn live_testnet4_send() {
    let dir = PathBuf::from(std::env::var("OB_LIVE_DIR").unwrap_or_else(|_| "/tmp/ob-live-testnet4".to_string()));
    let urls: Vec<String> = NetworkMode::Testnet4.default_esplora().iter().map(|url| url.to_string()).collect();
    let chain = crate::EsploraChain::new(&urls, NetworkMode::Testnet4.bitcoin()).unwrap();
    let node = Node::open_with(
        &dir,
        load_catalog(NetworkMode::Testnet4).unwrap(),
        NodeConfig {
            network: NetworkMode::Testnet4,
            confirmations: 1,
            public_server: true,
            chain: Box::new(chain),
            escrow: EscrowConfig::default(),
        },
    )
    .unwrap();
    node.sync_buyer_wallet().unwrap();
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    println!("source wallet: {wallet:#}");
    let Ok(to) = std::env::var("OB_SEND_TO") else {
        return;
    };
    let sats: u64 = std::env::var("OB_SEND_SATS").unwrap().parse().unwrap();
    let address = address_for(&to, NetworkMode::Testnet4).unwrap();
    let fee_rate = node.pay_fee_rate().unwrap();
    let mut psbt = node.build_payment(&address, sats, fee_rate, false).unwrap();
    assert!(node.buyer_wallet().sign(&mut psbt).unwrap(), "not fully signed");
    let tx = psbt.extract_tx().unwrap();
    let fee = node.buyer_wallet().wallet.calculate_fee(&tx).unwrap().to_sat();
    node.chain.broadcast(&tx).unwrap();
    node.buyer_wallet().wallet.apply_unconfirmed_txs([(tx.clone(), crate::buyer_orders::now_secs())]);
    node.buyer_wallet().persist().unwrap();
    println!(
        "sent {sats} sats to {to} in {} (fee {fee} sats, {} vB, {} sat/vB)",
        tx.compute_txid(),
        tx.vsize(),
        fee_rate.to_sat_per_kwu() as f64 / 250.0
    );
    let (_, after) = request(&node, "GET", "/wallet", "");
    println!("source wallet after: {:#}", after["balance"]);
}
