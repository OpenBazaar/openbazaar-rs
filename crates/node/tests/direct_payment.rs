//! One real `ob-node` process takes a direct bitcoin order for a demo store
//! from checkout to Paid.
//!
//! The in-process tests (`payment_tests.rs`) cover the same order on a mock
//! chain and step the watcher by hand. Here the binary's own watcher asks a
//! stand-in Esplora server on loopback, through its real chain client, and
//! someone outside the node pays the order's address in two parts. The node's
//! own wallet sends nothing: the stand-in serves no wallet scan.

#[allow(dead_code)]
mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::{Address, ScriptBuf};
use common::{wait_for, RealNode};
use serde_json::{json, Value};

const PASSPHRASE: &str = "correct horse battery";

#[test]
fn a_real_node_takes_a_part_payment_then_the_rest_and_marks_the_order_paid() {
    let chain = Esplora::start();
    let limit = Duration::from_secs(30);
    let node = RealNode::start("buyer", &chain.url, &["--confirmations", "2"], limit);
    // The answer carries the new wallet's phrase, so it is not printed.
    let (status, _) = node.request("POST", "/setup", &json!({ "passphrase": PASSPHRASE }).to_string());
    assert_eq!(status, 200, "POST /setup");
    let placed = checkout(&node);
    assert_eq!(placed["status"], "Awaiting payment", "{placed}");
    let address = placed["payment"]["address"].as_str().unwrap();
    let total = placed["totalSats"].as_u64().unwrap();
    let path = format!("/orders/{}", placed["orderId"].as_str().unwrap());
    let order_when = |step: &str, ready: &dyn Fn(&Value) -> bool| {
        wait_for(step, limit, || {
            let (_, order) = node.request("GET", &path, "");
            if ready(&order) { Ok(order) } else { Err(order.to_string()) }
        })
    };

    chain.pay(address, total - 1_000);
    let part = order_when("the part payment to be seen", &|order| order["receivedSats"] == total - 1_000);
    assert_eq!(part["status"], "Awaiting payment", "{part}");
    chain.mine();
    let part = order_when("the part payment to confirm", &|order| order["confirmations"] == 1);
    assert_eq!((&part["status"], &part["receivedSats"]), (&json!("Awaiting payment"), &json!(total - 1_000)), "{part}");

    chain.pay(address, 1_000);
    let seen = order_when("the rest to be seen", &|order| order["receivedSats"] == total);
    assert_eq!((&seen["status"], &seen["confirmations"]), (&json!("Payment seen"), &json!(0)), "{seen}");
    chain.mine();
    let seen = order_when("the rest to confirm once", &|order| order["confirmations"] == 1);
    assert_eq!(seen["status"], "Payment seen", "two confirmations are required: {seen}");
    chain.mine();
    let paid = order_when("the order to be paid", &|order| order["status"] == "Paid");
    assert_eq!((&paid["receivedSats"], &paid["confirmations"]), (&json!(total), &json!(2)), "{paid}");
    assert_eq!(paid["txids"].as_array().unwrap().len(), 2, "{paid}");
    assert!(!node.data_dir().join("tor-client").exists(), "the node opened the Tor client that dials the introductions");
}

/// Check out one demo-store listing for direct payment. Returns the order.
fn checkout(node: &RealNode) -> Value {
    let (status, catalog) = node.request("GET", "/catalog", "");
    assert_eq!(status, 200, "{catalog}");
    let listings = catalog["listings"].as_array().unwrap();
    let listing = listings.iter().find(|item| item["title"] == "Paperback novel" && item["seller"] == "Marlow Goods").expect("the demo listing");
    let body = json!({
        "shipTo": { "name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202" },
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = node.request("POST", "/checkout", &body.to_string());
    assert_eq!(status, 200, "{placed}");
    placed["orders"][0].clone()
}

/// One payment the stand-in chain knows: who it pays, how much, and the block it is in.
struct Payment {
    txid: String,
    script: ScriptBuf,
    sats: u64,
    height: Option<u32>,
}

#[derive(Default)]
struct Chain {
    tip: u32,
    payments: Vec<Payment>,
}

/// A stand-in Esplora server with only what the node's watcher asks for:
/// the tip height and the transactions that pay a script. Every other
/// request is answered 404, so a wallet scan or a broadcast fails at once.
struct Esplora {
    url: String,
    chain: Arc<Mutex<Chain>>,
}

impl Esplora {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let chain = Arc::new(Mutex::new(Chain { tip: 100, payments: Vec::new() }));
        let served = Arc::clone(&chain);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = match answer(&served.lock().unwrap(), path) {
                    Some(body) => ("200 OK", body),
                    None => ("404 Not Found", String::new()),
                };
                let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        Self { url, chain }
    }

    /// Someone outside the node pays `sats` to `address`, unconfirmed.
    fn pay(&self, address: &str, sats: u64) {
        let script = address.parse::<Address<_>>().unwrap().assume_checked().script_pubkey();
        let mut chain = self.chain.lock().unwrap();
        let txid = format!("{:064x}", chain.payments.len() + 1);
        chain.payments.push(Payment { txid, script, sats, height: None });
    }

    /// Mine every unconfirmed payment into the next block.
    fn mine(&self) {
        let mut chain = self.chain.lock().unwrap();
        chain.tip += 1;
        let tip = chain.tip;
        for payment in chain.payments.iter_mut().filter(|payment| payment.height.is_none()) {
            payment.height = Some(tip);
        }
    }
}

/// The body for `GET path`, or `None` for a path this server does not have.
fn answer(chain: &Chain, path: &str) -> Option<String> {
    if path == "/blocks/tip/height" {
        return Some(chain.tip.to_string());
    }
    let wanted = path.strip_prefix("/scripthash/")?.strip_suffix("/txs")?;
    let paying = chain.payments.iter().filter(|payment| format!("{:x}", sha256::Hash::hash(payment.script.as_bytes())) == wanted);
    let txs: Vec<Value> = paying
        .map(|payment| {
            json!({
                "txid": payment.txid,
                "version": 2,
                "locktime": 0,
                "vin": [],
                "vout": [{ "scriptpubkey": payment.script.to_hex_string(), "value": payment.sats }],
                "size": 0,
                "weight": 0,
                "fee": 0,
                "status": { "confirmed": payment.height.is_some(), "block_height": payment.height },
            })
        })
        .collect();
    Some(Value::Array(txs).to_string())
}
