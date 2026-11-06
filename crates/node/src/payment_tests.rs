//! Payments against a mocked chain source. Nothing here touches the internet.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bdk_wallet::chain::TxUpdate;
use bdk_wallet::Update;
use bitcoin::absolute::LockTime;
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use serde_json::{json, Value};

use crate::chain::{AddressTx, ChainSource, WalletRequest};
use crate::http::dispatch;
use crate::network::{address_for, NetworkMode, ServeOptions};
use crate::{load_catalog, Node, NodeConfig, ESCROW_SPEND_FEE_SATS};

#[derive(Default)]
struct Chain {
    tip: Mutex<u32>,
    by_script: Mutex<HashMap<ScriptBuf, Vec<AddressTx>>>,
    wallet_txs: Mutex<Vec<Transaction>>,
    broadcasts: Mutex<Vec<Transaction>>,
    fee_rate: Mutex<f64>,
    fail_broadcast: AtomicBool,
}

#[derive(Clone)]
struct MockChain(Arc<Chain>);

impl MockChain {
    fn new() -> Self {
        let chain = Chain::default();
        *chain.tip.lock().unwrap() = 100;
        *chain.fee_rate.lock().unwrap() = 2.0;
        Self(Arc::new(chain))
    }

    /// Someone outside this node pays `sats` to `address`. Returns the fake txid.
    fn pay_external(&self, address: &str, sats: u64) -> Txid {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&n.to_le_bytes());
        bytes[31] = 0xee;
        let txid = Txid::from_byte_array(bytes);
        let script = address_any(address).script_pubkey();
        self.0.by_script.lock().unwrap().entry(script).or_default().push(AddressTx {
            txid,
            outputs: vec![(0, sats)],
            height: None,
        });
        txid
    }

    /// Mine every mempool transaction into the next block.
    fn mine(&self) {
        let mut tip = self.0.tip.lock().unwrap();
        *tip += 1;
        for txs in self.0.by_script.lock().unwrap().values_mut() {
            for tx in txs.iter_mut().filter(|tx| tx.height.is_none()) {
                tx.height = Some(*tip);
            }
        }
    }

    /// Give the node wallet a coin it can spend.
    fn fund_wallet(&self, address: &Address, sats: u64) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([n as u8 ^ 0x5a; 32]),
                    vout: n as u32,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: address.script_pubkey(),
            }],
        };
        self.0.wallet_txs.lock().unwrap().push(tx);
    }

    fn broadcasts(&self) -> Vec<Transaction> {
        self.0.broadcasts.lock().unwrap().clone()
    }
}

impl ChainSource for MockChain {
    fn tip_height(&self) -> Result<u32, String> {
        Ok(*self.0.tip.lock().unwrap())
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        Ok(self
            .0
            .by_script
            .lock()
            .unwrap()
            .get(&address.script_pubkey())
            .cloned()
            .unwrap_or_default())
    }

    fn fee_rate(&self) -> Result<f64, String> {
        Ok(*self.0.fee_rate.lock().unwrap())
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        if self.0.fail_broadcast.swap(false, Ordering::Relaxed) {
            return Err("mock relay is down".to_string());
        }
        let txid = tx.compute_txid();
        let mut index = self.0.by_script.lock().unwrap();
        for (vout, out) in tx.output.iter().enumerate() {
            let entry = index.entry(out.script_pubkey.clone()).or_default();
            entry.push(AddressTx {
                txid,
                outputs: vec![(vout as u32, out.value.to_sat())],
                height: None,
            });
        }
        self.0.broadcasts.lock().unwrap().push(tx.clone());
        Ok(())
    }

    fn wallet_update(&self, _: WalletRequest) -> Result<Update, String> {
        let txs = self.0.wallet_txs.lock().unwrap().clone();
        let mut tx_update = TxUpdate::default();
        tx_update.seen_ats = txs.iter().map(|tx| (tx.compute_txid(), 1)).collect::<HashSet<_>>();
        tx_update.txs = txs.into_iter().map(Arc::new).collect();
        Ok(Update {
            tx_update,
            ..Default::default()
        })
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("ob-pay-{nanos}-{}-{n}", std::process::id())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &Path, network: NetworkMode, chain: &MockChain, confirmations: u32) -> Node {
    Node::open_with(
        dir,
        load_catalog(network).unwrap(),
        NodeConfig {
            network,
            confirmations,
            public_server: false,
            chain: Box::new(chain.clone()),
        },
    )
    .unwrap()
}

fn testnet() -> (TempDir, MockChain, Node) {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open(&dir.0, NetworkMode::Testnet4, &chain, 1);
    (dir, chain, node)
}

fn address_any(text: &str) -> Address {
    text.parse::<Address<_>>().unwrap().assume_checked()
}

fn hash_of(node: &Node, title: &str, seller: &str) -> String {
    node.catalog
        .listings
        .iter()
        .find(|listing| listing.title == title && listing.seller == seller)
        .unwrap()
        .content_hash()
        .unwrap()
        .to_hex()
}

fn request(node: &Node, method: &str, path: &str, body: &str) -> (String, Value) {
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\nX-Ob-Pair: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        node.pairing_token(),
        body.len(),
    );
    let response = String::from_utf8(dispatch(node, raw.as_bytes())).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.lines().next().unwrap().to_string();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn ship_to() -> Value {
    json!({"name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202"})
}

/// One checkout with one package per `(title, seller, settlement)`. Returns the orders.
fn checkout(node: &Node, packages: &[(&str, &str, &str)]) -> Vec<Value> {
    let packages: Vec<Value> = packages
        .iter()
        .map(|(title, seller, settlement)| {
            json!({"settlement": settlement, "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]})
        })
        .collect();
    let body = json!({"shipTo": ship_to(), "packages": packages});
    let (status, value) = request(node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    value["orders"].as_array().unwrap().clone()
}

fn order(node: &Node, id: &str) -> Value {
    let (status, value) = request(node, "GET", &format!("/orders/{id}"), "");
    assert!(status.contains("200"), "{status} {value}");
    value
}

/// `POST /orders/{id}/pay`, then run the queued payment the way the pay
/// worker would. Returns the HTTP status and the order afterwards.
fn pay(node: &Node, id: &str, body: &str) -> (String, Value) {
    let (status, value) = request(node, "POST", &format!("/orders/{id}/pay"), body);
    if !status.contains("202") {
        return (status, value);
    }
    assert_eq!(value["paying"], true, "{value}");
    assert_eq!(node.run_pay_jobs(), 1);
    let after = order(node, id);
    assert_eq!(after["paying"], false);
    (status, after)
}

fn fund_node_wallet(node: &Node, chain: &MockChain, sats: u64) {
    let address = node.buyer_wallet().fresh_address().unwrap();
    chain.fund_wallet(&address, sats);
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
            for key in ["address", "buyerPayout", "vendorPayout"] {
                let value = escrow[0]["escrow"][key].as_str().unwrap().to_string();
                assert!(seen.insert(value.clone()), "{key} {value} was handed out twice");
            }
        }
    }
    assert_eq!(seen.len(), 2 * 3 * 4);
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

    let (_dir, _chain, test) = testnet();
    let placed = checkout(&test, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let terms = &placed[0]["escrow"];
    assert_eq!(terms["demo"], true);
    assert_eq!(terms["unsafe"], true);
    assert!(terms["warning"].as_str().unwrap().contains("testnet4 coins only"));
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
    let escrow_id = placed[0]["orderId"].as_str().unwrap();
    pay(&node, escrow_id, "{}");
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

#[test]
fn escrow_funding_holds_the_refund_before_it_broadcasts() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let terms = &placed[0]["escrow"];
    let funding = terms["fundingSats"].as_u64().unwrap();
    assert_eq!(funding, placed[0]["totalSats"].as_u64().unwrap() + ESCROW_SPEND_FEE_SATS);
    let address = address_for(terms["address"].as_str().unwrap(), NetworkMode::Testnet4).unwrap();

    chain.0.fail_broadcast.store(true, Ordering::Relaxed);
    let (_, held) = pay(&node, id, "{}");
    assert!(held["payError"].as_str().unwrap().contains("relay is down"), "{held}");
    assert_eq!(held["escrow"]["refundHeld"], true, "the refund is saved before the broadcast");
    assert_eq!(held["status"], "Awaiting funding");
    let stored_funding = held["escrow"]["fundingTx"].as_str().unwrap().to_string();

    let (_, paid) = pay(&node, id, "{}");
    assert!(paid["payTxid"].is_string(), "{paid}");
    assert!(paid.get("payError").is_none());
    assert_eq!(paid["escrow"]["refundHeld"], true);
    let sent = chain.broadcasts();
    assert_eq!(sent.len(), 1);
    let funding_tx = &sent[0];
    let stored: Transaction = deserialize_hex(&stored_funding).unwrap();
    assert_eq!(funding_tx.compute_txid(), stored.compute_txid(), "the retry sends the transaction the refund spends");

    let after = order(&node, id);
    let outpoint: OutPoint = after["escrow"]["outpoint"].as_str().unwrap().parse().unwrap();
    assert_eq!(outpoint.txid, funding_tx.compute_txid());
    let output = &funding_tx.output[outpoint.vout as usize];
    assert_eq!(output.script_pubkey, address.script_pubkey());
    assert_eq!(output.value.to_sat(), funding);
    let refund: Transaction = deserialize_hex(after["escrow"]["refundTx"].as_str().unwrap()).unwrap();
    assert_eq!(refund.input.len(), 1);
    assert_eq!(refund.input[0].previous_output, outpoint);
    assert_eq!(refund.input[0].witness.len(), 1, "a single key-path signature");
    let buyer_payout = address_for(after["escrow"]["buyerPayout"].as_str().unwrap(), NetworkMode::Testnet4).unwrap();
    assert_eq!(refund.output[0].script_pubkey, buyer_payout.script_pubkey());
    assert_eq!(refund.output[0].value.to_sat(), funding - ESCROW_SPEND_FEE_SATS);
    assert!(node.buyer_wallet().wallet.is_mine(buyer_payout.script_pubkey()));
    let timeout = after["escrow"]["timeoutHeight"].as_u64().unwrap() as u32;
    assert_eq!(timeout, 100 + crate::ESCROW_TIMEOUT_BLOCKS);
    assert_eq!(refund.lock_time, LockTime::from_height(timeout).unwrap());

    node.watch_once().unwrap();
    assert_eq!(order(&node, id)["status"], "Funding seen");
    chain.mine();
    node.watch_once().unwrap();
    assert_eq!(order(&node, id)["status"], "Funded");
}

#[test]
fn escrow_funded_from_elsewhere_is_flagged_without_a_refund() {
    let (_dir, chain, node) = testnet();
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "escrow")]);
    let id = placed[0]["orderId"].as_str().unwrap();
    let terms = &placed[0]["escrow"];
    let address = terms["address"].as_str().unwrap();
    let funding = terms["fundingSats"].as_u64().unwrap();

    chain.pay_external(address, funding - 10);
    chain.mine();
    node.watch_once().unwrap();
    let short = order(&node, id);
    assert_eq!(short["status"], "Awaiting funding");
    assert_eq!(short["receivedSats"], funding - 10);

    chain.pay_external(address, funding);
    chain.mine();
    node.watch_once().unwrap();
    let flagged = order(&node, id);
    assert_eq!(flagged["status"], "Funded without a refund");
    assert_eq!(flagged["escrow"]["refundHeld"], false);
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
        },
    )
    .unwrap();
    node.sync_buyer_wallet().unwrap();
    let (_, wallet) = request(&node, "GET", "/wallet", "");
    println!("node wallet on testnet4: {wallet:#}");
    let confirmed = wallet["balance"]["totalSats"].as_u64().unwrap();
    if confirmed < 20_000 {
        println!("fund {} with at least 20000 testnet4 sats, then run again", wallet["receiveAddress"]);
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
