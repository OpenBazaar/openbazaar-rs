//! Monero direct pay through the HTTP routes, on a mocked daemon.

use std::path::Path;
use std::sync::Arc;

use openbazaar_xmr::amount::piconero_for_sats;
use openbazaar_xmr::mock::MockChain as XmrChainMock;
use openbazaar_xmr::{address_for, MoneroAddress, XmrNetwork};
use serde_json::{json, Value};

use crate::harness::{hash_of, open_node, order, request, ship_to, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::network::NetworkMode;
use crate::spend_tickets::TICKET_TTL;
use crate::{Egress, Node};

const SATS_PER_XMR: u64 = 300_000;
const XMR: u64 = 1_000_000_000_000;
const NET: XmrNetwork = XmrNetwork::Stagenet;

struct Setup {
    node: Node,
    xmr: Arc<XmrChainMock>,
    btc: MockChain,
    dir: TempDir,
}

fn open(dir: &Path, btc: &MockChain, xmr: &Arc<XmrChainMock>) -> Node {
    let node = open_node(dir, NetworkMode::Testnet4, btc, 1, unreachable_escrow());
    node.set_xmr_chain(xmr.clone()).unwrap();
    node.xmr.rate.set_for_tests(Some(SATS_PER_XMR));
    node
}

/// A node with Monero on, Marlow Goods accepting it, every wallet synced.
fn setup() -> Setup {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let xmr = Arc::new(XmrChainMock::new(NET));
    let node = open(&dir.0, &btc, &xmr);
    let (status, stores) = request(&node, "PUT", "/xmr/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    assert!(status.contains("200"), "{status} {stores}");
    node.xmr_sync_once().unwrap();
    Setup { node, xmr, btc, dir }
}

fn place(node: &Node, title: &str, seller: &str, settlement: &str, rail: &str) -> (String, Value) {
    let package = json!({"settlement": settlement, "rail": rail, "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]});
    request(node, "POST", "/checkout", &json!({"shipTo": ship_to(), "packages": [package]}).to_string())
}

/// One Monero order from Marlow Goods. Returns its id and payment.
fn xmr_order(node: &Node) -> (String, Value) {
    let (status, value) = place(node, "Paperback novel", "Marlow Goods", "direct", "xmr");
    assert!(status.contains("200"), "{status} {value}");
    let placed = &value["orders"][0];
    (placed["orderId"].as_str().unwrap().to_string(), placed["payment"].clone())
}

fn subaddress(payment: &Value) -> MoneroAddress {
    address_for(payment["subaddress"].as_str().unwrap(), NET).unwrap()
}

fn piconero(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}

fn quote_and_ticket(node: &Node, id: &str) -> (Value, String) {
    let (status, quote) = request(node, "POST", &format!("/xmr/orders/{id}/pay/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    let body = json!({"quote": quote["quote"], "passphrase": PASSPHRASE}).to_string();
    let (status, ticket) = request(node, "POST", "/xmr/spend-tickets", &body);
    assert!(status.contains("200"), "{status} {ticket}");
    (quote, ticket["ticket"].as_str().unwrap().to_string())
}

fn pay(node: &Node, id: &str, ticket: &str) -> (String, Value) {
    request(node, "POST", &format!("/xmr/orders/{id}/pay"), &json!({"ticket": ticket}).to_string())
}

#[test]
fn each_order_gets_its_own_subaddress_and_an_exact_price_in_piconero() {
    let Setup { node, xmr, btc, dir } = setup();
    let (_, first) = xmr_order(&node);
    let (_, second) = xmr_order(&node);
    let price = first["priceSats"].as_u64().unwrap();
    assert_eq!(piconero(&first["amountPiconero"]), piconero_for_sats(price, SATS_PER_XMR).unwrap());
    assert_eq!(first["rail"], "xmr");
    assert_eq!(first["network"], "stagenet");
    assert_eq!(first["confirmationsNeeded"], 10);
    assert!(first.get("address").is_none(), "the bitcoin watcher and pay worker skip it");
    assert!(first["uri"].as_str().unwrap().starts_with(&format!("monero:{}?tx_amount=", first["subaddress"].as_str().unwrap())));
    drop(node);
    let node = open(&dir.0, &btc, &xmr);
    let (_, third) = xmr_order(&node);
    let mut seen: Vec<&str> = [&first, &second, &third].iter().map(|payment| payment["subaddress"].as_str().unwrap()).collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 3, "a subaddress is never handed out twice, across restarts too");

    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert_eq!(catalog["xmr"], json!({"enabled": true, "network": "stagenet"}));
    let accepts = |seller: &str| catalog["listings"].as_array().unwrap().iter().find(|listing| listing["seller"] == seller).unwrap()["acceptsXmr"].clone();
    assert_eq!(accepts("Marlow Goods"), true);
    assert_eq!(accepts("Hale Paper"), false);
}

#[test]
fn an_order_waits_for_all_of_it_then_is_seen_then_paid_after_ten_confirmations() {
    let Setup { node, xmr, btc: _btc, dir: _dir } = setup();
    let (id, payment) = xmr_order(&node);
    let due = piconero(&payment["amountPiconero"]);
    let to = subaddress(&payment);

    xmr.mine_to(&to, due / 2);
    node.xmr_sync_once().unwrap();
    node.watch_once().unwrap();
    let now = order(&node, &id);
    assert_eq!(now["status"], "Awaiting payment", "{now}");
    assert_eq!(piconero(&now["payment"]["receivedPiconero"]), due / 2);
    assert_eq!(piconero(&now["payment"]["remainingPiconero"]), due - due / 2);

    xmr.mine_to(&to, due - due / 2);
    node.xmr_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Payment seen");
    xmr.mine(8);
    node.xmr_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Payment seen", "9 confirmations of the last part");
    xmr.mine(1);
    node.xmr_sync_once().unwrap();
    let paid = order(&node, &id);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(paid["txids"].as_array().unwrap().len(), 2);
    assert_eq!(piconero(&paid["payment"]["overpaidPiconero"]), 0);

    let (over, payment) = xmr_order(&node);
    xmr.mine_to(&subaddress(&payment), piconero(&payment["amountPiconero"]) + 7);
    xmr.mine(9);
    node.xmr_sync_once().unwrap();
    let paid = order(&node, &over);
    assert_eq!(paid["status"], "Paid");
    assert_eq!(piconero(&paid["payment"]["overpaidPiconero"]), 7);
}

#[test]
fn escrow_stores_that_do_not_accept_it_and_missing_prices_refuse_monero() {
    let Setup { mut node, btc, xmr: _xmr, dir: _dir } = setup();
    let refused = |node: &Node, title: &str, seller: &str, settlement: &str, rail: &str| {
        let (status, value) = place(node, title, seller, settlement, rail);
        assert!(status.contains("400"), "{status} {value}");
        value["error"].as_str().unwrap().to_string()
    };
    assert_eq!(refused(&node, "Paperback novel", "Marlow Goods", "escrow", "xmr"), super::checkout::ESCROW_IS_BITCOIN_ONLY);
    assert!(refused(&node, "Letterpress cards", "Hale Paper", "direct", "xmr").contains("does not accept Monero"));
    assert!(refused(&node, "Paperback novel", "Marlow Goods", "direct", "eth").contains("btc, xmr, or ln"));
    node.xmr.rate = super::rate::XmrRate::new(Egress { proxy: None, live_rates: false });
    assert!(refused(&node, "Paperback novel", "Marlow Goods", "direct", "xmr").contains("no XMR price"));
    assert_eq!(request(&node, "GET", "/rates", "").1["xmr"], Value::Null);
    assert_eq!(node.orders().on_network().count(), 0, "nothing was placed");

    let dir = TempDir::new();
    let off = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    request(&off, "PUT", "/xmr/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    assert!(refused(&off, "Paperback novel", "Marlow Goods", "direct", "xmr").contains("Monero is off"));
    assert_eq!(request(&off, "GET", "/network", "").1["xmrBackend"]["backend"], "none");
    let (status, _) = place(&off, "Paperback novel", "Marlow Goods", "direct", "btc");
    assert!(status.contains("200"), "the bitcoin rail is unchanged");
}

#[test]
fn a_locked_node_takes_monero_orders_and_marks_them_paid_but_never_spends() {
    let Setup { node, xmr, btc: _btc, dir: _dir } = setup();
    let (unpaid, _) = xmr_order(&node);
    node.lock.lock();
    let (id, payment) = xmr_order(&node);
    let (status, wallet) = request(&node, "GET", "/xmr/wallet", "");
    assert!(status.contains("200") && wallet["keys"] == "ready", "{status} {wallet}");
    let (status, fresh) = request(&node, "POST", "/xmr/wallet/subaddress", "");
    assert!(status.contains("200") && fresh["address"].as_str().unwrap().starts_with('7'), "{status} {fresh}");

    xmr.mine_to(&subaddress(&payment), piconero(&payment["amountPiconero"]));
    xmr.mine(9);
    node.xmr_sync_once().unwrap();
    assert_eq!(order(&node, &id)["status"], "Paid", "the view key is in the receive tier");

    let (status, value) = request(&node, "POST", &format!("/xmr/orders/{unpaid}/pay/quote"), "");
    assert!(status.contains("423"), "{status} {value}");
    let (status, value) = pay(&node, &unpaid, "anything");
    assert!(status.contains("423"), "{status} {value}");
    assert!(xmr.published().is_empty());
}

#[test]
fn the_node_wallet_pays_one_order_with_a_ticket_bound_to_it() {
    let Setup { node, xmr, btc, dir: _dir } = setup();
    let (status, wallet) = request(&node, "GET", "/xmr/wallet", "");
    assert!(status.contains("200"), "{status} {wallet}");
    let primary = address_for(wallet["primaryAddress"].as_str().unwrap(), NET).unwrap();
    xmr.mine_to(&primary, 5 * XMR);
    xmr.mine(10);
    node.xmr_sync_once().unwrap();
    let balance = request(&node, "GET", "/xmr/wallet", "").1["balance"].clone();
    assert_eq!(balance["unlockedPiconero"], (5 * XMR).to_string());
    assert_eq!(balance["unlockedXmr"], "5");

    let (a, payment) = xmr_order(&node);
    let (b, other) = xmr_order(&node);
    let (quote, ticket) = quote_and_ticket(&node, &a);
    assert_eq!(quote["orderId"], a.as_str());
    assert_eq!(quote["amountPiconero"], payment["amountPiconero"]);
    assert_eq!(quote["destination"], payment["subaddress"]);
    assert!(piconero(&quote["feeCapPiconero"]) > 0);

    let (status, value) = pay(&node, &b, &ticket);
    assert!(status.contains("409") && value["error"].as_str().unwrap().contains("another order"), "{status} {value}");
    let (status, _) = pay(&node, &a, &ticket);
    assert!(status.contains("409"), "a ticket tried on another order is gone");
    let (_, ticket) = quote_and_ticket(&node, &a);
    let (status, value) = pay(&node, &a, &ticket);
    assert!(status.contains("202") && value["paying"] == true, "{status} {value}");
    assert_eq!(node.run_xmr_pay_jobs(), 1);
    let sent = order(&node, &a);
    assert!(sent["payTxid"].is_string() && sent["paying"] == false, "{sent}");
    assert_eq!(xmr.published().len(), 1);
    assert!(btc.broadcasts().is_empty(), "no bitcoin moved");
    let (status, _) = pay(&node, &a, &ticket);
    assert!(status.contains("409"), "a ticket is redeemed once");

    xmr.mine(1);
    node.xmr_sync_once().unwrap();
    assert_eq!(order(&node, &a)["status"], "Payment seen");
    xmr.mine(9);
    node.xmr_sync_once().unwrap();
    assert_eq!(order(&node, &a)["status"], "Paid");
    let (status, _) = request(&node, "POST", &format!("/xmr/orders/{a}/pay/quote"), "");
    assert!(status.contains("409"), "paid once only");

    let (_, ticket) = quote_and_ticket(&node, &b);
    xmr.mine_to(&subaddress(&other), 1_000);
    node.xmr_sync_once().unwrap();
    let (status, value) = pay(&node, &b, &ticket);
    assert!(status.contains("409") && value["error"] == crate::pay_order::TERMS_CHANGED, "{status} {value}");
    let (_, ticket) = quote_and_ticket(&node, &b);
    node.lock.advance(TICKET_TTL);
    let (status, _) = pay(&node, &b, &ticket);
    assert!(status.contains("409"), "an expired ticket is refused");
    assert_eq!(xmr.published().len(), 1);
}

#[test]
fn network_and_rates_say_what_the_monero_backend_sees() {
    let Setup { node, xmr, btc: _btc, dir: _dir } = setup();
    let (_, network) = request(&node, "GET", "/network", "");
    let backend = &network["xmrBackend"];
    assert_eq!(backend["backend"], "own-node", "{backend}");
    assert_eq!(backend["network"], "stagenet");
    assert_eq!(backend["tip"], xmr.height());
    assert!(backend["reveals"].as_str().unwrap().contains("No one else"));
    assert_eq!(backend["scannedHeight"], xmr.height());
    let (_, rates) = request(&node, "GET", "/rates", "");
    assert_eq!(rates["xmr"]["satsPerXmr"], SATS_PER_XMR);
    assert_eq!(rates["xmr"]["decimals"], 12);
    let (_, stores) = request(&node, "GET", "/xmr/stores", "");
    assert_eq!(stores["stores"][0], json!({"seller": "Hale Paper", "acceptsXmr": false}));
    let (status, _) = request(&node, "PUT", "/xmr/stores", &json!({"seller": "Nobody", "accept": true}).to_string());
    assert!(status.contains("404"));
}

/// Live stagenet run. Opt in with:
///
/// `OB_XMR_DAEMON=http://127.0.0.1:38081 OB_LIVE_DIR=/tmp/ob-live-xmr cargo test -p ob-node live_stagenet -- --ignored --nocapture`
///
/// Add `OB_PROXY=socks5h://127.0.0.1:9050` for a remote or .onion node. It
/// prints the node wallet's Monero address. Fund it from a stagenet faucet,
/// run again with the same `OB_LIVE_DIR` once 10 blocks have passed, and it
/// pays one order to a store on the same node and waits until it is Paid.
#[test]
#[ignore]
fn live_stagenet() {
    use std::time::{Duration, Instant};
    let dir = std::path::PathBuf::from(std::env::var("OB_LIVE_DIR").unwrap_or_else(|_| "/tmp/ob-live-xmr".to_string()));
    let urls = vec![std::env::var("OB_XMR_DAEMON").expect("OB_XMR_DAEMON")];
    let config = openbazaar_xmr::DaemonConfig::new(NET, &urls, std::env::var("OB_PROXY").ok()).unwrap();
    let btc = MockChain::new();
    let node = open_node(&dir, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    node.set_xmr_chain(Arc::new(openbazaar_xmr::DaemonChain::new(config).unwrap())).unwrap();
    if let Some(sats) = std::env::var("OB_XMR_SATS_PER_XMR").ok().and_then(|sats| sats.parse().ok()) {
        node.xmr.rate.set_for_tests(Some(sats));
    }
    let sync = |node: &Node| loop {
        node.xmr_sync_once().unwrap();
        let wallet = request(node, "GET", "/xmr/wallet", "").1;
        if wallet["scannedHeight"] == wallet["tip"] {
            return wallet;
        }
    };
    let wallet = sync(&node);
    println!("node Monero wallet on stagenet: {wallet:#}");
    if piconero(&wallet["balance"]["unlockedPiconero"]) < XMR / 20 {
        println!("fund {} with at least 0.05 stagenet XMR and run again after 10 blocks", wallet["primaryAddress"]);
        return;
    }
    request(&node, "PUT", "/xmr/stores", &json!({"seller": "Marlow Goods", "accept": true}).to_string());
    let (id, payment) = xmr_order(&node);
    println!("order {id} pays {}", payment["uri"]);
    let (_, ticket) = quote_and_ticket(&node, &id);
    let (status, value) = pay(&node, &id, &ticket);
    assert!(status.contains("202"), "{status} {value}");
    node.run_xmr_pay_jobs();
    let sent = order(&node, &id);
    assert!(sent["payTxid"].is_string(), "{sent}");
    println!("broadcast {}", sent["payTxid"]);
    let start = Instant::now();
    loop {
        sync(&node);
        let now = order(&node, &id);
        println!("{} after {:?}: {} confirmation(s)", now["status"], start.elapsed(), now["confirmations"]);
        if now["status"] == "Paid" {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3600), "not confirmed within an hour");
        std::thread::sleep(Duration::from_secs(30));
    }
}
