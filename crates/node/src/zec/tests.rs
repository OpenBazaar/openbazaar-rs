//! Zcash direct pay through the HTTP routes, on a mock chain of real notes.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openbazaar_zec::amount::zatoshi_for_sats;
use openbazaar_zec::mock::MockChain as ZecChainMock;
use openbazaar_zec::{Pool, CONFIRMATIONS};
use serde_json::{json, Value};

use crate::harness::{hash_of, open_node, order, request, ship_to, unreachable_escrow, MockChain, TempDir};
use crate::network::NetworkMode;
use crate::{Egress, Node};

/// 1 ZEC = 0.0122936 BTC.
const SATS_PER_ZEC: u64 = 1_229_360;

struct Setup {
    node: Node,
    zec: Arc<ZecChainMock>,
    btc: MockChain,
    dir: TempDir,
}

fn open(dir: &Path, btc: &MockChain, zec: &Arc<ZecChainMock>) -> Node {
    let node = open_node(dir, NetworkMode::Testnet4, btc, 1, unreachable_escrow());
    node.set_zec_chain(zec.clone()).unwrap();
    node.zec.rate.set_for_tests(Some(SATS_PER_ZEC));
    node
}

/// A node with Zcash on, Marlow Goods accepting it, every wallet synced.
fn setup() -> Setup {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let zec = Arc::new(ZecChainMock::new(NetworkMode::Testnet4.zcash()));
    let node = open(&dir.0, &btc, &zec);
    let (status, stores) = request(&node, "PUT", "/zec/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    assert!(status.contains("200"), "{status} {stores}");
    node.zec_sync_once().unwrap();
    Setup { node, zec, btc, dir }
}

fn place(node: &Node, title: &str, seller: &str, settlement: &str, rail: &str) -> (String, Value) {
    let package = json!({"settlement": settlement, "rail": rail, "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]});
    request(node, "POST", "/checkout", &json!({"shipTo": ship_to(), "packages": [package]}).to_string())
}

/// One Zcash order from Marlow Goods. Returns its id and payment.
fn zec_order(node: &Node) -> (String, Value) {
    let (status, value) = place(node, "Paperback novel", "Marlow Goods", "direct", "zec");
    assert!(status.contains("200"), "{status} {value}");
    let placed = &value["orders"][0];
    (placed["orderId"].as_str().unwrap().to_string(), placed["payment"].clone())
}

fn zatoshi(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}

fn address(payment: &Value) -> &str {
    payment["zcashAddress"].as_str().unwrap()
}

#[test]
fn each_order_gets_its_own_address_and_an_exact_price_in_zatoshi() {
    let Setup { node, zec, btc, dir } = setup();
    let (_, first) = zec_order(&node);
    let (_, second) = zec_order(&node);
    let price = first["priceSats"].as_u64().unwrap();
    assert_eq!(zatoshi(&first["amountZatoshi"]), zatoshi_for_sats(price, SATS_PER_ZEC).unwrap());
    assert_eq!((&first["rail"], &first["network"], &first["pool"]), (&json!("zec"), &json!("testnet"), &json!("ironwood")));
    assert_eq!(first["confirmationsNeeded"], CONFIRMATIONS);
    assert!(first.get("address").is_none(), "the bitcoin watcher and pay worker skip it");
    assert!(address(&first).starts_with("utest1"));
    assert!(first["uri"].as_str().unwrap().starts_with(&format!("zcash:{}?amount=", address(&first))));
    {
        let slot = node.zec.store("Marlow Goods").unwrap();
        let keys = slot.wallet.as_ref().unwrap().keys();
        let index = first["addressIndex"].as_u64().unwrap() as u32;
        assert_eq!(keys.address(index), address(&first), "an Orchard-protocol address of the seller's own key");
    }
    drop(node);
    let node = open(&dir.0, &btc, &zec);
    let (_, third) = zec_order(&node);
    let mut seen: Vec<&str> = [&first, &second, &third].iter().map(|payment| address(payment)).collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 3, "an address is never handed out twice, across restarts too");
    assert!(third["addressIndex"].as_u64() > second["addressIndex"].as_u64());

    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert_eq!(catalog["zec"], json!({"enabled": true, "network": "testnet", "pool": "ironwood"}));
    let accepts = |seller: &str| catalog["listings"].as_array().unwrap().iter().find(|listing| listing["seller"] == seller).unwrap()["acceptsZec"].clone();
    assert_eq!(accepts("Marlow Goods"), true);
    assert_eq!(accepts("Hale Paper"), false);
}

#[test]
fn an_order_waits_for_all_of_it_then_is_seen_then_paid_after_thirty_confirmations() {
    let Setup { node, zec, btc: _btc, dir: _dir } = setup();
    let (id, payment) = zec_order(&node);
    let due = zatoshi(&payment["amountZatoshi"]);
    let to = address(&payment);

    zec.pay(to, due / 2);
    zec.mine(1);
    node.zec_sync_once().unwrap();
    node.watch_once().unwrap();
    let now = order(&node, &id);
    assert_eq!(now["status"], "Awaiting payment", "{now}");
    assert_eq!(zatoshi(&now["payment"]["receivedZatoshi"]), due / 2);
    assert_eq!(zatoshi(&now["payment"]["remainingZatoshi"]), due - due / 2);

    zec.pay(to, due - due / 2);
    zec.mine(1);
    node.zec_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Payment seen");
    zec.mine(CONFIRMATIONS - 2);
    node.zec_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Payment seen", "29 confirmations of the last part");
    zec.mine(1);
    node.zec_sync_once().unwrap();
    let paid = order(&node, &id);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(paid["confirmations"], CONFIRMATIONS);
    assert_eq!(paid["txids"].as_array().unwrap().len(), 2);
    assert_eq!(zatoshi(&paid["payment"]["overpaidZatoshi"]), 0);

    let (over, payment) = zec_order(&node);
    zec.pay(address(&payment), zatoshi(&payment["amountZatoshi"]) + 7);
    zec.mine(CONFIRMATIONS);
    node.zec_sync_once().unwrap();
    let paid = order(&node, &over);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(zatoshi(&paid["payment"]["overpaidZatoshi"]), 7);
}

#[test]
fn a_payment_whose_blocks_are_replaced_before_paid_is_gone_until_it_is_mined_again() {
    let Setup { node, zec, btc: _btc, dir: _dir } = setup();
    let (id, payment) = zec_order(&node);
    let due = zatoshi(&payment["amountZatoshi"]);
    zec.pay(address(&payment), due);
    zec.mine(3);
    node.zec_sync_once().unwrap();
    let seen = order(&node, &id);
    assert_eq!((&seen["status"], &seen["confirmations"]), (&json!("Payment seen"), &json!(3)), "{seen}");

    zec.reorg(3);
    zec.mine(4);
    node.zec_sync_once().unwrap();
    let gone = order(&node, &id);
    assert_eq!((&gone["status"], &gone["confirmations"]), (&json!("Awaiting payment"), &json!(0)), "{gone}");
    assert_eq!(zatoshi(&gone["payment"]["receivedZatoshi"]), 0);
    assert_eq!(zatoshi(&gone["payment"]["remainingZatoshi"]), due);
    assert_eq!(gone["txids"], json!([]));

    zec.pay(address(&payment), due);
    zec.mine(CONFIRMATIONS - 1);
    node.zec_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Payment seen", "29 confirmations on the new chain");
    zec.mine(1);
    node.zec_sync_once().unwrap();
    let paid = order(&node, &id);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(paid["txids"].as_array().unwrap().len(), 1, "only the payment that stayed: {paid}");
}

#[test]
fn only_ironwood_pays_an_order() {
    let Setup { node, zec, btc: _btc, dir: _dir } = setup();
    let (id, payment) = zec_order(&node);
    // Orchard takes no new value since NU6.3; only a server that ignores consensus sends this.
    zec.pay_in_pool(address(&payment), zatoshi(&payment["amountZatoshi"]), Pool::Orchard);
    zec.mine(CONFIRMATIONS);
    node.zec_sync_once().unwrap();
    let now = order(&node, &id);
    assert_eq!(now["status"], "Awaiting payment", "{now}");
    assert_eq!(zatoshi(&now["payment"]["receivedZatoshi"]), 0);
}

#[test]
fn escrow_stores_that_do_not_accept_it_and_missing_prices_refuse_zcash() {
    let Setup { mut node, btc, zec: _zec, dir: _dir } = setup();
    let refused = |node: &Node, title: &str, seller: &str, settlement: &str, rail: &str| {
        let (status, value) = place(node, title, seller, settlement, rail);
        assert!(status.contains("400"), "{status} {value}");
        value["error"].as_str().unwrap().to_string()
    };
    assert_eq!(refused(&node, "Paperback novel", "Marlow Goods", "escrow", "zec"), super::checkout::ESCROW_IS_BITCOIN_ONLY);
    assert!(refused(&node, "Letterpress cards", "Hale Paper", "direct", "zec").contains("does not accept Zcash"));
    assert!(refused(&node, "Paperback novel", "Marlow Goods", "direct", "eth").contains("btc, xmr, ln, or zec"));
    node.zec.rate = super::rate::ZecRate::new(Egress { proxy: None, live_rates: false });
    assert!(refused(&node, "Paperback novel", "Marlow Goods", "direct", "zec").contains("no ZEC price"));
    assert_eq!(request(&node, "GET", "/rates", "").1["zec"], Value::Null);
    assert_eq!(node.orders().on_network().count(), 0, "nothing was placed");

    let dir = TempDir::new();
    let off = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    request(&off, "PUT", "/zec/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    assert!(refused(&off, "Paperback novel", "Marlow Goods", "direct", "zec").contains("Zcash is off"));
    let (_, network) = request(&off, "GET", "/network", "");
    assert_eq!((&network["zecBackend"]["enabled"], &network["zecBackend"]["backend"]), (&json!(false), &json!("none")));
    let (status, _) = place(&off, "Paperback novel", "Marlow Goods", "direct", "btc");
    assert!(status.contains("200"), "the bitcoin rail is unchanged");
}

#[test]
fn a_btc_price_older_than_the_zec_limit_does_not_price_a_zcash_order() {
    let Setup { node, btc: _btc, zec: _zec, dir: _dir } = setup();
    node.zec.rate.set_for_tests(None);
    node.zec.rate.set_usd_for_tests(1229.36);
    node.rates.set_answer_for_tests(100_000.0, super::rate::USABLE - Duration::from_secs(60));
    let (_, payment) = zec_order(&node);
    assert_eq!(payment["satsPerZec"], 1_229_360);
    assert_eq!(request(&node, "GET", "/rates", "").1["zec"]["satsPerZec"], 1_229_360);

    // Coinbase has failed for longer than the limit; the node still holds its last answer.
    node.rates.set_answer_for_tests(100_000.0, super::rate::USABLE + Duration::from_secs(60));
    let (status, refused) = place(&node, "Paperback novel", "Marlow Goods", "direct", "zec");
    assert!(status.contains("400") && refused["error"].as_str().unwrap().contains("no ZEC price"), "{status} {refused}");
    let (_, rates) = request(&node, "GET", "/rates", "");
    assert_eq!(rates["zec"], Value::Null);
    assert_eq!(rates["currencies"]["USD"], 100_000.0, "the bitcoin estimate still shows the old answer");
    let (status, _) = place(&node, "Paperback novel", "Marlow Goods", "direct", "btc");
    assert!(status.contains("200"), "the bitcoin rail is unchanged");
}

#[test]
fn a_locked_node_takes_zcash_orders_and_marks_them_paid() {
    let Setup { node, zec, btc, dir } = setup();
    node.lock.lock();
    let (id, payment) = zec_order(&node);
    zec.pay(address(&payment), zatoshi(&payment["amountZatoshi"]));
    zec.mine(CONFIRMATIONS);
    node.zec_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Paid", "the viewing key is in the receive tier");

    drop(node);
    let node = open(&dir.0, &btc, &zec);
    node.lock.lock();
    let (_, again) = zec_order(&node);
    assert!(again["addressIndex"].as_u64() > payment["addressIndex"].as_u64(), "the key and the address count came back while locked");
}

#[test]
fn an_order_is_refused_until_the_wallet_has_read_the_chain_once() {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let zec = Arc::new(ZecChainMock::new(NetworkMode::Testnet4.zcash()));
    let node = open(&dir.0, &btc, &zec);
    request(&node, "PUT", "/zec/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    zec.set_offline(true);
    assert!(node.zec_sync_once().is_err());
    let (status, refused) = place(&node, "Paperback novel", "Marlow Goods", "direct", "zec");
    assert!(status.contains("400") && refused["error"] == super::checkout::NOT_SYNCED, "{status} {refused}");
    assert_eq!(node.orders().on_network().count(), 0, "no address was handed out for the first sync to skip");

    zec.set_offline(false);
    node.zec_sync_once().unwrap();
    let (id, payment) = zec_order(&node);
    // Paid while the indexer is away for longer than the first sync looks back.
    zec.set_offline(true);
    zec.pay(address(&payment), zatoshi(&payment["amountZatoshi"]));
    zec.mine(131);
    zec.set_offline(false);
    node.zec_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Paid", "a wallet that has synced once goes on from where it stopped");
}

#[test]
fn only_demo_stores_opt_in_and_the_choice_is_signed_and_kept() {
    let Setup { node, zec, btc, dir } = setup();
    let store = node.seller_store("Marlow Goods").unwrap();
    assert!(store.details.as_ref().unwrap().accepts_zec);
    store.verify().unwrap();
    drop(node);
    let node = open(&dir.0, &btc, &zec);
    assert!(node.accepts_zec("Marlow Goods"), "the choice survives a restart");
    node.seller_store("Marlow Goods").unwrap().verify().unwrap();

    let secret = node.store_secret(&node.own_store).expect("unlocked");
    node.publish_own(openbazaar_protocol::ContentRating::General, crate::own_store::Rails::default(), &secret).unwrap();
    let own = node.own_record().expect("the own store is published").name;
    let (status, value) = request(&node, "PUT", "/zec/stores", &json!({"seller": own, "accept": true}).to_string());
    assert!(status.contains("409") && value["error"] == super::OWN_STORE_LATER, "{status} {value}");
    assert!(!node.own_record().unwrap().details.unwrap().accepts_zec, "no published record sets the Zcash bit yet");

    let (status, _) = request(&node, "PUT", "/zec/stores", &json!({"seller": "Nobody", "accept": true}).to_string());
    assert!(status.contains("404"), "{status}");
    let (status, value) = request(&node, "PUT", "/zec/stores", &json!({"seller": "Marlow Goods"}).to_string());
    assert!(status.contains("400"), "{status} {value}");

    let (status, stores) = request(&node, "PUT", "/zec/stores", &json!({"seller": "Marlow Goods", "accept": false}).to_string());
    assert!(status.contains("200"), "{status} {stores}");
    let marlow = stores["stores"].as_array().unwrap().iter().find(|store| store["seller"] == "Marlow Goods").unwrap().clone();
    assert_eq!(marlow["acceptsZec"], false);
    assert!(!node.seller_store("Marlow Goods").unwrap().details.unwrap().accepts_zec);
}

#[test]
fn network_and_rates_say_what_the_zcash_backend_sees() {
    let Setup { node, zec, btc: _btc, dir: _dir } = setup();
    zec.mine(3);
    node.zec_sync_once().unwrap();
    let (_, network) = request(&node, "GET", "/network", "");
    let backend = &network["zecBackend"];
    assert_eq!((&backend["enabled"], &backend["backend"]), (&json!(true), &json!("mock")));
    assert_eq!((&backend["network"], &backend["pool"]), (&json!("testnet"), &json!("ironwood")));
    assert_eq!(backend["scannedHeight"], zec.height());
    assert_eq!(backend["tip"], zec.height());
    let (_, rates) = request(&node, "GET", "/rates", "");
    assert_eq!((&rates["zec"]["base"], &rates["zec"]["satsPerZec"], &rates["zec"]["decimals"]), (&json!("ZEC"), &json!(SATS_PER_ZEC), &json!(8)));
}
