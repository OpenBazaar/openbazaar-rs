//! Lightning direct pay through the HTTP routes, on mocked Lightning nodes.

use std::path::Path;
use std::sync::Arc;

use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::mock::MockRail;
use openbazaar_lightning::LightningRail;
use serde_json::{json, Value};

use crate::harness::{hash_of, open_node, order, request, ship_to, unlock, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::network::NetworkMode;
use crate::Node;

use super::LnFlags;

struct Setup {
    node: Node,
    rail: Arc<MockRail>,
    btc: MockChain,
    dir: TempDir,
}

fn open(dir: &Path, btc: &MockChain, rail: &Arc<MockRail>) -> Node {
    let node = open_node(dir, NetworkMode::Testnet4, btc, 1, unreachable_escrow());
    let mut flags = LnFlags::default();
    let args = [
        "--lightning",
        "--ln-chain",
        "bitcoind:u:p@127.0.0.1:18443",
        "--lsp",
        "0371d6fd7d75de2d0372d03ea00e8bacdacb50c27d0eaea0a76a0622eff1f5ef2b@127.0.0.1:9735",
        "--ln-p2p-gossip",
    ];
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        flags.take(flag, |_| iter.next().map(|text| text.to_string()).ok_or_else(|| "needs a value".to_string())).unwrap().unwrap();
    }
    node.enable_lightning(flags.plan(NetworkMode::Testnet4).unwrap().unwrap()).unwrap();
    node.set_lightning(rail.clone()).unwrap();
    node
}

fn accept(node: &Node, seller: &str, acknowledge: bool) -> (String, Value) {
    let body = json!({"seller": seller, "accept": true, "acknowledgeLsp": acknowledge}).to_string();
    request(node, "PUT", "/ln/stores", &body)
}

/// Lightning on, Marlow Goods taking it after the LSP notice.
fn setup() -> Setup {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let rail = Arc::new(MockRail::new(Network::Signet));
    let node = open(&dir.0, &btc, &rail);
    let (status, stores) = accept(&node, "Marlow Goods", true);
    assert!(status.contains("200"), "{status} {stores}");
    Setup { node, rail, btc, dir }
}

fn place(node: &Node, title: &str, seller: &str, settlement: &str) -> (String, Value) {
    let package = json!({"settlement": settlement, "rail": "ln", "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]});
    request(node, "POST", "/checkout", &json!({"shipTo": ship_to(), "packages": [package]}).to_string())
}

fn ln_order(node: &Node) -> (String, Value) {
    let (status, value) = place(node, "Paperback novel", "Marlow Goods", "direct");
    assert!(status.contains("200"), "{status} {value}");
    let placed = &value["orders"][0];
    (placed["orderId"].as_str().unwrap().to_string(), placed["payment"].clone())
}

/// An order whose invoice another Lightning node issued, as a remote seller's would be.
fn remote_order(node: &Node, seller: &MockRail) -> (String, String) {
    let (id, payment) = ln_order(node);
    let invoice = seller.receive(payment["amountMsat"].as_u64().unwrap(), "remote", 600).unwrap();
    node.orders()
        .update(&id, |order| {
            let payment = order.payment.as_mut().unwrap();
            payment["invoice"] = json!(invoice.bolt11);
            payment["paymentHash"] = json!(invoice.payment_hash);
            payment["issuer"] = json!("remote");
        })
        .unwrap();
    (id, invoice.payment_hash)
}

fn quote_and_ticket(node: &Node, id: &str) -> (Value, String) {
    let (status, quote) = request(node, "POST", &format!("/ln/orders/{id}/pay/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    let body = json!({"quote": quote["quote"], "passphrase": PASSPHRASE}).to_string();
    let (status, ticket) = request(node, "POST", "/ln/spend-tickets", &body);
    assert!(status.contains("200"), "{status} {ticket}");
    (quote, ticket["ticket"].as_str().unwrap().to_string())
}

fn pay(node: &Node, id: &str, ticket: &str) -> (String, Value) {
    request(node, "POST", &format!("/ln/orders/{id}/pay"), &json!({"ticket": ticket}).to_string())
}

#[test]
fn a_store_takes_lightning_only_after_the_lsp_notice_and_signs_it() {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let rail = Arc::new(MockRail::new(Network::Signet));
    let node = open(&dir.0, &btc, &rail);
    let (status, refused) = accept(&node, "Marlow Goods", false);
    assert!(status.contains("409"), "{status} {refused}");
    assert!(refused["lspNotice"].as_str().unwrap().contains("sees the amount and time"));
    assert_eq!(node.accepts_lightning("Marlow Goods"), false);
    let (status, stores) = accept(&node, "Marlow Goods", true);
    assert!(status.contains("200"), "{status} {stores}");
    assert_eq!(stores["lspAcknowledged"], true);
    let (status, _) = accept(&node, "Hale Paper", false);
    assert!(status.contains("200"), "the notice is shown once");

    let store = node.seller_store("Marlow Goods").unwrap();
    assert!(store.details.as_ref().unwrap().accepts_lightning);
    store.verify().expect("the record is signed again with the flag");
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert_eq!(catalog["lightning"], json!({"enabled": true, "running": true, "network": "signet"}));
    let accepts = |seller: &str| catalog["listings"].as_array().unwrap().iter().find(|listing| listing["seller"] == seller).unwrap()["acceptsLightning"].clone();
    assert_eq!(accepts("Marlow Goods"), true);
    assert_eq!(accepts("North Kitchen"), false);

    drop(node);
    let node = open(&dir.0, &btc, &rail);
    assert!(node.accepts_lightning("Marlow Goods"), "the choice survives a restart");
    let (status, _) = request(&node, "PUT", "/ln/stores", &json!({"seller": "Nobody", "accept": true}).to_string());
    assert!(status.contains("404"));
}

#[test]
fn a_locked_node_issues_invoices_and_marks_orders_paid() {
    let Setup { node, rail, btc, dir: _dir } = setup();
    rail.set_lsp_fee(3_000);
    node.lock.lock();
    let (id, payment) = ln_order(&node);
    assert_eq!(payment["rail"], "ln");
    assert!(payment.get("address").is_none(), "the bitcoin watcher and pay worker skip it");
    assert_eq!(payment["amountMsat"], order(&node, &id)["totalSats"].as_u64().unwrap() * 1_000);
    assert_eq!(payment["jit"], true);
    assert!(payment["uri"].as_str().unwrap().starts_with("lightning:LNTBS"), "{payment}");
    assert_eq!(order(&node, &id)["status"], "Awaiting payment");

    assert!(rail.settle(payment["paymentHash"].as_str().unwrap()), "paid from an outside wallet");
    assert_eq!(node.ln_update_orders(), 1);
    let paid = order(&node, &id);
    assert_eq!(paid["status"], "Paid", "{paid}");
    assert_eq!(paid["payment"]["lspFeeMsat"], 3_000);
    assert_eq!(paid["payment"]["receivedMsat"], payment["amountMsat"].as_u64().unwrap() - 3_000);
    assert_eq!(node.ln_update_orders(), 0);
    assert!(btc.broadcasts().is_empty());

    let (status, value) = request(&node, "POST", &format!("/ln/orders/{id}/pay/quote"), "");
    assert!(status.contains("423"), "{status} {value}");
}

#[test]
fn the_node_pays_an_order_once_with_a_ticket_bound_to_it() {
    let Setup { node, rail, btc: _btc, dir: _dir } = setup();
    let seller = Arc::new(rail.linked());
    let (own, _) = ln_order(&node);
    let (status, value) = request(&node, "POST", &format!("/ln/orders/{own}/pay/quote"), "");
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("another Lightning wallet"), "{status} {value}");

    let (a, hash) = remote_order(&node, &seller);
    let (b, _) = remote_order(&node, &seller);
    let (status, value) = request(&node, "POST", &format!("/ln/orders/{a}/pay/quote"), "");
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("Lightning balance"), "{status} {value}");
    rail.set_lightning_sats(1_000_000);

    node.lock.lock();
    let (status, _) = request(&node, "POST", &format!("/ln/orders/{a}/pay/quote"), "");
    assert!(status.contains("423"));
    let (status, _) = pay(&node, &a, "anything");
    assert!(status.contains("423"));
    unlock(&node);

    let (status, value) = request(&node, "POST", &format!("/ln/orders/{a}/pay"), "{}");
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("spend ticket"), "{status} {value}");
    let (quote, ticket) = quote_and_ticket(&node, &a);
    assert_eq!(quote["paymentHash"], hash.as_str());
    assert_eq!(quote["feeCapMsat"], super::pay::fee_cap(quote["amountMsat"].as_u64().unwrap()));
    let (status, value) = pay(&node, &b, &ticket);
    assert!(status.contains("409") && value["error"].as_str().unwrap().contains("another order"), "{status} {value}");
    let (_, ticket) = quote_and_ticket(&node, &a);
    let (status, value) = pay(&node, &a, &ticket);
    assert!(status.contains("202"), "{status} {value}");
    assert_eq!(value["status"], "Paid", "{value}");
    assert!(matches!(seller.incoming(&hash), openbazaar_lightning::Incoming::Paid { .. }));
    let (status, _) = pay(&node, &a, &ticket);
    assert!(status.contains("409"), "a ticket is redeemed once");
    let (status, _) = request(&node, "POST", &format!("/ln/orders/{a}/pay/quote"), "");
    assert!(status.contains("409"), "paid once only");

    let (_, ticket) = quote_and_ticket(&node, &b);
    rail.fail_next_pay(openbazaar_lightning::LnError::Failed("no route".into()));
    let (status, value) = pay(&node, &b, &ticket);
    assert!(status.contains("502") || status.contains("503"), "{status} {value}");
    let failed = order(&node, &b);
    assert_eq!(failed["paying"], false);
    assert!(failed["payError"].as_str().unwrap().contains("no route"), "{failed}");

    node.lock.lock();
    assert!(node.lock.ln_tickets.redeem(&ticket, &b, node.lock.now()).is_err(), "locking clears tickets");
}

#[test]
fn channel_work_needs_the_unlocked_wallet_and_the_passphrase() {
    let Setup { node, rail, btc: _btc, dir: _dir } = setup();
    let peer = super::options::MUTINYNET_LSP;
    let open_body = |passphrase: &str| json!({"peer": peer, "sats": 100_000, "passphrase": passphrase}).to_string();
    let (status, value) = request(&node, "POST", "/ln/channels", &open_body("wrong"));
    assert!(status.contains("400"), "{status} {value}");
    assert!(rail.channels().is_empty());
    let (status, value) = request(&node, "POST", "/ln/channels", &open_body(PASSPHRASE));
    assert!(status.contains("200"), "{status} {value}");
    let id = value["channelId"].as_str().unwrap().to_string();
    let (_, status_value) = request(&node, "GET", "/ln/status", "");
    assert_eq!(status_value["channels"][0]["capacitySats"], 100_000);
    assert_eq!(status_value["lsp"]["connected"], true);
    assert!(status_value["reveals"][0].as_str().unwrap().contains("LSP"));

    node.lock.lock();
    let (status, _) = request(&node, "POST", &format!("/ln/channels/{id}/close"), &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("423"));
    let (status, _) = request(&node, "POST", "/ln/sweep", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("423"));
    let (status, value) = request(&node, "POST", "/ln/address", "");
    assert!(status.contains("200") && value["address"].is_string(), "receiving works while locked");
    unlock(&node);
    let (status, value) = request(&node, "POST", &format!("/ln/channels/{id}/close"), &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("200"), "{status} {value}");
    assert!(rail.channels().is_empty());
    let (status, value) = request(&node, "POST", "/ln/sweep", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("400") && value["error"].as_str().unwrap().contains("another network"), "{status} {value}");
}

#[test]
fn escrow_stores_that_do_not_take_it_and_a_node_without_lightning_refuse_it() {
    let Setup { node, rail: _rail, btc, dir: _dir } = setup();
    let refused = |node: &Node, title: &str, seller: &str, settlement: &str| {
        let (status, value) = place(node, title, seller, settlement);
        assert!(status.contains("400"), "{status} {value}");
        value["error"].as_str().unwrap().to_string()
    };
    assert_eq!(refused(&node, "Paperback novel", "Marlow Goods", "escrow"), super::checkout::ESCROW_STAYS_ON_CHAIN);
    assert!(refused(&node, "Letterpress cards", "Hale Paper", "direct").contains("does not accept Lightning"));
    assert_eq!(node.orders().on_network().count(), 0, "nothing was placed");

    let dir = TempDir::new();
    let off = open_node(&dir.0, NetworkMode::Testnet4, &btc, 1, unreachable_escrow());
    assert!(refused(&off, "Paperback novel", "Marlow Goods", "direct").contains("Lightning is off"));
    assert_eq!(request(&off, "GET", "/ln/status", "").1, json!({"enabled": false}));
    assert_eq!(request(&off, "GET", "/network", "").1["lightning"], json!({"enabled": false}));
    let (status, _) = accept(&off, "Marlow Goods", true);
    assert!(status.contains("409"));
}

#[test]
fn mainnet_refuses_lightning_at_every_entry_point() {
    let dir = TempDir::new();
    let btc = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Mainnet, &btc, 3, unreachable_escrow());
    let blocked = super::MAINNET_BLOCKED;

    let mut flags = LnFlags::default();
    flags.on = true;
    assert_eq!(flags.plan(NetworkMode::Mainnet).unwrap_err(), blocked);
    let mut local = LnFlags::default();
    let args = ["--lightning", "--ln-chain", "bitcoind:u:p@127.0.0.1:18443", "--no-lsp", "--ln-p2p-gossip"];
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        local.take(flag, |_| iter.next().map(|text| text.to_string()).ok_or_else(|| "needs a value".to_string())).unwrap().unwrap();
    }
    let config = local.plan(NetworkMode::Testnet4).unwrap().unwrap();
    assert_eq!(node.enable_lightning(config).unwrap_err(), blocked, "a test-network config does not slip in either");
    assert_eq!(node.set_lightning(Arc::new(MockRail::new(Network::Bitcoin))).unwrap_err(), blocked);
    assert!(node.ln.config().is_none() && node.ln.rail().is_none());

    for path in ["/ln/status", "/ln/stores"] {
        assert_eq!(request(&node, "GET", path, "").1, json!({"enabled": false, "blocked": blocked}), "{path}");
    }
    let (_, network) = request(&node, "GET", "/network", "");
    assert_eq!(network["lightning"]["blocked"], blocked);
    assert_eq!(network["lightningBackend"]["blocked"], blocked);
    let posts = [
        ("PUT", "/ln/stores", json!({"seller": "Marlow Goods", "accept": true, "acknowledgeLsp": true})),
        ("POST", "/ln/address", json!({})),
        ("POST", "/ln/channels", json!({"peer": super::options::MUTINYNET_LSP, "sats": 100_000, "passphrase": PASSPHRASE})),
        ("POST", "/ln/channels/abc/close", json!({"passphrase": PASSPHRASE})),
        ("POST", "/ln/sweep", json!({"passphrase": PASSPHRASE})),
        ("POST", "/ln/spend-tickets", json!({"quote": "q", "passphrase": PASSPHRASE})),
        ("POST", "/ln/orders/abc/pay/quote", json!({})),
        ("POST", "/ln/orders/abc/pay", json!({"ticket": "t", "confirm": true})),
    ];
    for (method, path, body) in posts {
        let (status, value) = request(&node, method, path, &body.to_string());
        assert!(status.contains("403") && value["error"] == blocked, "{method} {path}: {status} {value}");
    }
    assert!(matches!(node.set_accepts_lightning("Marlow Goods", true, true), Err(super::AcceptError::Mainnet)));

    // A record that claims Lightning is not offered, and checkout refuses it.
    node.catalog_mut().stores.iter_mut().find(|store| store.name == "Marlow Goods").unwrap().details.as_mut().unwrap().accepts_lightning = true;
    assert!(!node.accepts_lightning("Marlow Goods"));
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert_eq!(catalog["lightning"]["blocked"], blocked);
    assert!(catalog["listings"].as_array().unwrap().iter().all(|listing| listing["acceptsLightning"] == false));
    let (status, value) = place(&node, "Paperback novel", "Marlow Goods", "direct");
    assert!(status.contains("400") && value["error"] == blocked, "{status} {value}");
    assert_eq!(node.orders().on_network().count(), 0, "nothing was placed");
    assert!(node.ln_payment(1_000, "order").is_err(), "no invoice is issued");

    // The own store does not sign the Lightning flag on mainnet.
    let secret = node.store_secret(&node.own_store).expect("unlocked");
    let rails = crate::own_store::Rails { lightning: true, ..crate::own_store::Rails::default() };
    node.publish_own(openbazaar_protocol::ContentRating::General, rails, &secret).unwrap();
    let own = node.own_record().expect("the own store is published");
    assert!(!own.details.as_ref().unwrap().accepts_lightning);
    own.verify().unwrap();
}

#[test]
fn test_networks_still_run_lightning() {
    let Setup { node, rail: _rail, btc: _btc, dir: _dir } = setup();
    assert!(!node.ln_blocked());
    let (_, status) = request(&node, "GET", "/ln/status", "");
    assert_eq!(status["enabled"], true);
    assert!(status.get("blocked").is_none());
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert!(catalog["lightning"].get("blocked").is_none());
    let (id, payment) = ln_order(&node);
    assert_eq!(payment["rail"], "ln", "{id}");
    for network in ["testnet4", "regtest"] {
        let dir = TempDir::new();
        let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, unreachable_escrow());
        let mut flags = LnFlags::default();
        let args = vec!["--lightning", "--ln-network", network, "--ln-chain", "bitcoind:u:p@127.0.0.1:18443", "--no-lsp", "--ln-p2p-gossip"];
        let mut iter = args.into_iter();
        while let Some(flag) = iter.next() {
            flags.take(flag, |_| iter.next().map(String::from).ok_or_else(|| "needs a value".to_string())).unwrap().unwrap();
        }
        node.enable_lightning(flags.plan(NetworkMode::Testnet4).unwrap().unwrap()).unwrap();
        assert_eq!(request(&node, "GET", "/ln/status", "").1["network"], network);
    }
}

#[test]
fn the_lightning_secret_is_written_once_and_readable_while_locked() {
    let Setup { node, rail: _rail, btc: _btc, dir: _dir } = setup();
    assert!(node.ln.secret().unwrap().is_none());
    assert!(node.ln_fill_keys().unwrap());
    let secret = node.ln.secret().unwrap().unwrap();
    node.lock.lock();
    assert_eq!(node.ln.secret().unwrap().unwrap(), secret, "the receive tier holds it");
    assert!(node.ln_fill_keys().unwrap());
    assert_eq!(request(&node, "GET", "/ln/status", "").1["keys"], "ready");
}
