//! The wallet lock over HTTP: 423 on every spend route, spend tickets,
//! auto-lock, backoff, setup, and settings.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{
    checkout, fund_node_wallet, hash_of, open_node, order, pay, request, ship_to, ticket, unlock, unreachable_escrow,
    MockChain, TempDir, PASSPHRASE,
};
use crate::network::NetworkMode;
use crate::spend_tickets::TICKET_TTL;
use crate::wallet_lock::{State, SLEEP_GAP, SPEND_COST};
use crate::{load_catalog, Node, NodeConfig};

fn testnet() -> (TempDir, MockChain, Node) {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, unreachable_escrow());
    (dir, chain, node)
}

/// A node on a new data dir, before any passphrase.
fn fresh(dir: &TempDir) -> Node {
    let mut config = NodeConfig::offline(NetworkMode::Testnet4);
    config.escrow = unreachable_escrow();
    Node::open_with(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap(), config).unwrap()
}

fn direct_order(node: &Node) -> String {
    checkout(node, &[("Paperback novel", "Marlow Goods", "direct")])[0]["orderId"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every route that spends, signs, reveals, or changes a security setting.
fn spend_routes(id: &str) -> Vec<(&'static str, String, String)> {
    let escrow = json!({"shipTo": ship_to(), "packages": [
        {"settlement": "escrow", "lines": [{"listingHash": "LISTING", "quantity": 1}]}
    ]});
    let single = json!({"shipTo": ship_to(), "settlement": "escrow", "lines": [{"listingHash": "LISTING", "quantity": 1}]});
    vec![
        ("POST", "/orders".into(), single.to_string()),
        ("POST", format!("/orders/{id}/pay"), json!({"ticket": "00".repeat(32)}).to_string()),
        ("POST", format!("/orders/{id}/pay/quote"), String::new()),
        ("POST", "/spend-tickets".into(), json!({"quote": "00", "passphrase": PASSPHRASE}).to_string()),
        ("POST", "/checkout".into(), escrow.to_string()),
        ("POST", format!("/orders/{id}/release"), String::new()),
        ("POST", format!("/orders/{id}/dispute"), json!({"memo": "never arrived"}).to_string()),
        ("POST", format!("/moderator/disputes/{}/rule", "00".repeat(16)), json!({"favor": "buyer"}).to_string()),
        ("POST", "/wallet/phrase".into(), json!({"passphrase": PASSPHRASE}).to_string()),
        ("PUT", "/lock/settings".into(), json!({"passphrase": PASSPHRASE, "autoLockMinutes": 60}).to_string()),
        ("POST", "/lock/passphrase".into(), json!({"current": PASSPHRASE, "new": "another long one"}).to_string()),
    ]
}

fn assert_all_locked(node: &Node, id: &str, code: &str) {
    let listing = hash_of(node, "Paperback novel", "Marlow Goods");
    for (method, path, body) in spend_routes(id) {
        let body = body.replace("LISTING", &listing);
        let (status, value) = request(node, method, &path, &body);
        assert!(status.contains("423"), "{method} {path}: {status} {value}");
        assert_eq!(value["code"], code, "{method} {path}");
    }
}

#[test]
fn every_spend_route_is_423_while_locked_and_before_setup() {
    let dir = TempDir::new();
    let node = fresh(&dir);
    assert_eq!(node.lock.state(), State::Setup, "a new data dir starts without a passphrase");
    let id = direct_order(&node);
    let orders = request(&node, "GET", "/orders", "").1;
    assert_all_locked(&node, &id, "setup_required");

    unlock(&node);
    request(&node, "POST", "/lock", "");
    assert_eq!(node.lock.state(), State::Locked);
    assert_all_locked(&node, &id, "locked");
    assert_eq!(request(&node, "GET", "/orders", "").1, orders, "no escrow order was opened");
    assert!(node.lock.spend_key().is_err());
}

#[test]
fn the_pairing_token_alone_never_pays() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let id = direct_order(&node);
    for body in ["", "{}", r#"{"confirm": true}"#, r#"{"ticket": ""}"#] {
        let (status, value) = request(&node, "POST", &format!("/orders/{id}/pay"), body);
        assert!(status.contains("400"), "{body}: {status}");
        assert!(value["error"].as_str().unwrap().contains("spend ticket"), "{value}");
    }
    assert_eq!(node.run_pay_jobs(), 0);
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn a_quote_shows_the_exact_terms_and_the_ticket_repeats_them() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let id = direct_order(&node);
    let address = order(&node, &id)["payment"]["address"].clone();
    let (status, quote) = request(&node, "POST", &format!("/orders/{id}/pay/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    assert_eq!(quote["orderId"], json!(id));
    assert_eq!(quote["amountSats"], order(&node, &id)["totalSats"]);
    assert_eq!(quote["address"], address);
    assert!(quote["feeCapSats"].as_u64().unwrap() > 0);
    assert_eq!(quote["expiresInSecs"], TICKET_TTL.as_secs());
    let body = json!({"quote": quote["quote"], "passphrase": PASSPHRASE}).to_string();
    let (status, ticket) = request(&node, "POST", "/spend-tickets", &body);
    assert!(status.contains("200"), "{status} {ticket}");
    for key in ["orderId", "amountSats", "feeCapSats", "address", "recipientScript"] {
        assert_eq!(ticket[key], quote[key], "{key}");
    }
    let (status, _) = request(&node, "POST", "/spend-tickets", &body);
    assert!(status.contains("409"), "a quote turns into one ticket");
    let wrong = json!({"quote": quote["quote"], "passphrase": "not it at all"}).to_string();
    let (status, value) = request(&node, "POST", "/spend-tickets", &wrong);
    assert!(status.contains("400") && value["code"] == "wrong_passphrase", "{status} {value}");
}

#[test]
fn a_ticket_cannot_be_replayed_changed_or_used_for_another_order() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 200_000);
    let first = direct_order(&node);
    let second = checkout(&node, &[("Letterpress cards", "Hale Paper", "direct")])[0]["orderId"]
        .as_str()
        .unwrap()
        .to_string();
    let pay_with = |id: &str, body: Value| request(&node, "POST", &format!("/orders/{id}/pay"), &body.to_string());

    // Another order: refused, and the ticket is gone.
    let stolen = ticket(&node, &first).unwrap();
    let (status, value) = pay_with(&second, json!({"ticket": stolen}));
    assert!(status.contains("409") && value["error"].as_str().unwrap().contains("another order"), "{status} {value}");
    let (status, _) = pay_with(&first, json!({"ticket": stolen}));
    assert!(status.contains("409"), "a ticket tried on another order is spent");

    // Changed: a forged ticket, or terms in the body.
    let good = ticket(&node, &first).unwrap();
    let mut forged = good.clone();
    forged.replace_range(..2, if &good[..2] == "00" { "01" } else { "00" });
    let (status, _) = pay_with(&first, json!({"ticket": forged}));
    assert!(status.contains("409"), "{status}");
    let (status, _) = pay_with(&first, json!({"ticket": good, "amountSats": 1}));
    assert!(status.contains("400"), "the client cannot name terms");

    // The order changes after the user confirmed it.
    let confirmed = ticket(&node, &first).unwrap();
    node.orders().update(&first, |order| order.total_sats += 1).unwrap();
    let (status, value) = pay_with(&first, json!({"ticket": confirmed}));
    assert!(status.contains("409") && value["error"].as_str().unwrap().contains("no longer matches"), "{status} {value}");
    node.orders().update(&first, |order| order.total_sats -= 1).unwrap();

    // Replay: the second use of a good ticket.
    let once = ticket(&node, &first).unwrap();
    let (status, _) = pay_with(&first, json!({"ticket": once}));
    assert!(status.contains("202"), "{status}");
    assert_eq!(node.run_pay_jobs(), 1);
    node.orders().update(&first, |order| order.pay_txid = None).unwrap();
    let (status, _) = pay_with(&first, json!({"ticket": once}));
    assert!(status.contains("409"), "{status}");
    assert_eq!(node.run_pay_jobs(), 0);
    assert_eq!(chain.broadcasts().len(), 1);
}

#[test]
fn tickets_expire_and_a_lock_drops_them() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let id = direct_order(&node);
    let late = ticket(&node, &id).unwrap();
    node.lock.advance(TICKET_TTL);
    let (status, _) = request(&node, "POST", &format!("/orders/{id}/pay"), &json!({"ticket": late}).to_string());
    assert!(status.contains("409"), "expired: {status}");

    let dropped = ticket(&node, &id).unwrap();
    request(&node, "POST", "/lock", "");
    unlock(&node);
    let (status, _) = request(&node, "POST", &format!("/orders/{id}/pay"), &json!({"ticket": dropped}).to_string());
    assert!(status.contains("409"), "a lock drops every ticket: {status}");
    assert!(chain.broadcasts().is_empty());
}

#[test]
fn the_worker_refuses_a_fee_above_the_confirmed_cap_and_a_lock_before_signing() {
    let (_dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let id = direct_order(&node);
    let body = json!({"ticket": ticket(&node, &id).unwrap()}).to_string();
    *chain.0.fee_rate.lock().unwrap() = 40.0;
    let (status, _) = request(&node, "POST", &format!("/orders/{id}/pay"), &body);
    assert!(status.contains("202"));
    node.run_pay_jobs();
    let after = order(&node, &id);
    assert!(after["payError"].as_str().unwrap().contains("above the"), "{after}");
    assert!(chain.broadcasts().is_empty());

    *chain.0.fee_rate.lock().unwrap() = 2.0;
    let body = json!({"ticket": ticket(&node, &id).unwrap()}).to_string();
    request(&node, "POST", &format!("/orders/{id}/pay"), &body);
    node.lock.lock();
    node.run_pay_jobs();
    assert!(order(&node, &id)["payError"].as_str().unwrap().contains("locked"), "a queued payment does not outlive the lock");
    assert!(chain.broadcasts().is_empty());

    unlock(&node);
    let (_, paid) = pay(&node, &id, "{}");
    assert!(paid["payTxid"].is_string(), "{paid}");
}

#[test]
fn the_wallet_locks_itself_after_the_idle_time_or_a_sleep() {
    let (_dir, _chain, node) = testnet();
    let status = |node: &Node| request(node, "GET", "/lock", "").1;
    assert_eq!(status(&node)["autoLockMinutes"], 15, "15 minutes by default");
    node.lock.advance(Duration::from_secs(14 * 60));
    assert_eq!(node.lock.state(), State::Unlocked);
    request(&node, "GET", "/wallet", "");
    request(&node, "GET", "/orders", "");
    node.lock.advance(Duration::from_secs(60));
    assert_eq!(node.lock.state(), State::Locked, "reads and polling do not keep the wallet open");
    assert!(node.lock.spend_key().is_err());

    unlock(&node);
    node.lock.advance(Duration::from_secs(14 * 60));
    request(&node, "POST", "/lock/activity", "");
    node.lock.advance(Duration::from_secs(14 * 60));
    assert_eq!(node.lock.state(), State::Unlocked, "user activity restarts the timer");
    assert!(status(&node)["lockInSecs"].as_u64().unwrap() <= 60);

    for minutes in [0, 61] {
        let body = json!({"passphrase": PASSPHRASE, "autoLockMinutes": minutes}).to_string();
        let (code, _) = request(&node, "PUT", "/lock/settings", &body);
        assert!(code.contains("400"), "{minutes} minutes");
    }
    let body = json!({"passphrase": PASSPHRASE, "autoLockMinutes": 1}).to_string();
    let (code, value) = request(&node, "PUT", "/lock/settings", &body);
    assert!(code.contains("200"), "{value}");
    node.lock.advance(Duration::from_secs(61));
    assert!(node.lock.tick(), "the auto-lock round locks after one idle minute");
    assert_eq!(node.lock.state(), State::Locked);

    // A sleep shorter than the idle time: the wall clock runs ahead of the monotonic one.
    unlock(&node);
    assert!(!node.lock.tick());
    node.lock.advance_wall(SLEEP_GAP + Duration::from_secs(1));
    assert!(node.lock.tick(), "waking from sleep locks");
    assert_eq!(node.lock.state(), State::Locked);
}

#[test]
fn wrong_passphrases_back_off_exponentially_across_restarts() {
    let dir = TempDir::new();
    let node = fresh(&dir);
    unlock(&node);
    node.lock.lock();
    let try_unlock = |node: &Node, passphrase: &str| {
        request(node, "POST", "/unlock", &json!({"passphrase": passphrase}).to_string())
    };
    for _ in 0..2 {
        let (status, value) = try_unlock(&node, "wrong guess");
        assert!(status.contains("400") && value["retryAfterSecs"] == 0, "{status} {value}");
    }
    let (_, value) = try_unlock(&node, "wrong guess");
    assert_eq!(value["retryAfterSecs"], 2);
    let (status, value) = try_unlock(&node, PASSPHRASE);
    assert!(status.contains("429") && value["code"] == "backoff", "even the right passphrase waits: {status}");
    node.lock.advance_wall(Duration::from_secs(2));
    let (_, value) = try_unlock(&node, "wrong guess");
    assert_eq!(value["retryAfterSecs"], 4, "the delay doubles");
    drop(node);

    let node = fresh(&dir);
    assert_eq!(node.lock.state(), State::Locked, "a restart starts locked");
    let (status, value) = try_unlock(&node, PASSPHRASE);
    assert!(status.contains("429"), "the counter survives a restart: {status} {value}");
    assert_eq!(request(&node, "GET", "/lock", "").1["failedAttempts"], 4);
    // The 2 s the first node's clock was moved, then the 4 s delay.
    node.lock.advance_wall(Duration::from_secs(6));
    let (status, value) = try_unlock(&node, PASSPHRASE);
    assert!(status.contains("200") && value["state"] == "unlocked", "{status} {value}");
    assert_eq!(value["failedAttempts"], 0, "a success resets the counter");

    // Step-up guesses count too.
    for _ in 0..3 {
        request(&node, "POST", "/wallet/phrase", &json!({"passphrase": "wrong guess"}).to_string());
    }
    let (status, _) = request(&node, "POST", "/wallet/phrase", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("429"), "{status}");
}

#[test]
fn setup_shows_the_phrase_once_and_checks_the_backup() {
    let dir = TempDir::new();
    let node = fresh(&dir);
    let (status, value) = request(&node, "POST", "/setup", &json!({"passphrase": "short"}).to_string());
    assert!(status.contains("400"), "{value}");
    let (status, setup) = request(&node, "POST", "/setup", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("200"), "{setup}");
    let phrase: Vec<String> = setup["phrase"].as_str().unwrap().split(' ').map(str::to_string).collect();
    assert_eq!(phrase.len(), 12);
    assert_eq!(setup["lock"]["state"], "unlocked");
    assert_eq!(setup["lock"]["backupConfirmed"], false);
    let (status, _) = request(&node, "POST", "/setup", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("400"), "setup runs once");

    let (_, revealed) = request(&node, "POST", "/wallet/phrase", &json!({"passphrase": PASSPHRASE}).to_string());
    assert_eq!(revealed["confirmWords"], setup["confirmWords"], "a reload during setup can still confirm");
    let positions: Vec<usize> = setup["confirmWords"].as_array().unwrap().iter().map(|n| n.as_u64().unwrap() as usize).collect();
    assert_eq!(positions.len(), 3);
    let words = |shift: usize| -> Vec<String> { positions.iter().map(|p| phrase[(p - 1 + shift) % 12].clone()).collect() };
    let (status, _) = request(&node, "POST", "/setup/confirm", &json!({"words": words(1)}).to_string());
    assert!(status.contains("400"));
    let (status, value) = request(&node, "POST", "/setup/confirm", &json!({"words": words(0)}).to_string());
    assert!(status.contains("200") && value["backupConfirmed"] == true, "{status} {value}");

    let (status, revealed) = request(&node, "POST", "/wallet/phrase", &json!({"passphrase": PASSPHRASE}).to_string());
    assert!(status.contains("200"));
    assert_eq!(revealed["phrase"], setup["phrase"]);
    assert_eq!(revealed["confirmWords"], json!([]), "nothing left to confirm");
    let lock = std::fs::read(dir.0.join("wallet.lock")).unwrap();
    assert_eq!(&lock[..4], b"OBW1");
    assert!(!std::fs::read(dir.0.join("wallets/node/spend.obv")).unwrap().windows(5).any(|w| w == phrase[0].as_bytes()));
}

#[test]
fn changing_the_passphrase_rewraps_only_the_lock() {
    let dir = TempDir::new();
    let node = fresh(&dir);
    unlock(&node);
    let spend_before = std::fs::read(dir.0.join("wallets/node/spend.obv")).unwrap();
    let lock_before = std::fs::read(dir.0.join("wallet.lock")).unwrap();
    let body = json!({"current": "not the passphrase", "new": "a brand new passphrase"}).to_string();
    let (status, _) = request(&node, "POST", "/lock/passphrase", &body);
    assert!(status.contains("400"));
    let body = json!({"current": PASSPHRASE, "new": "a brand new passphrase"}).to_string();
    let (status, value) = request(&node, "POST", "/lock/passphrase", &body);
    assert!(status.contains("200"), "{value}");
    assert_ne!(std::fs::read(dir.0.join("wallet.lock")).unwrap(), lock_before);
    assert_eq!(std::fs::read(dir.0.join("wallets/node/spend.obv")).unwrap(), spend_before, "the seeds are not rewritten");
    assert!(!dir.0.join("wallet.tmp").exists());
    node.lock.lock();
    assert!(node.unlock_wallet(Some(PASSPHRASE)).is_err());
    node.unlock_wallet(Some("a brand new passphrase")).unwrap();
}

#[test]
fn a_remembered_device_unlocks_without_the_passphrase_but_spends_still_step_up() {
    let (dir, chain, node) = testnet();
    fund_node_wallet(&node, &chain, 100_000);
    let (status, _) = request(&node, "POST", "/unlock", r#"{"remembered": true}"#);
    assert!(status.contains("400"), "nothing remembered yet");
    let body = json!({"passphrase": PASSPHRASE, "rememberDevice": true}).to_string();
    let (status, value) = request(&node, "PUT", "/lock/settings", &body);
    assert!(status.contains("200") && value["rememberDevice"] == true, "{value}");
    assert!(dir.0.join("wallet.remember").exists());
    node.lock.lock();
    let (status, value) = request(&node, "POST", "/unlock", r#"{"remembered": true}"#);
    assert!(status.contains("200") && value["state"] == "unlocked", "{status} {value}");

    let id = direct_order(&node);
    let (_, quote) = request(&node, "POST", &format!("/orders/{id}/pay/quote"), "");
    let (status, _) = request(&node, "POST", "/spend-tickets", &json!({"quote": quote["quote"]}).to_string());
    assert!(status.contains("400"), "a step-up still needs the passphrase");

    let body = json!({"passphrase": PASSPHRASE, "rememberDevice": false}).to_string();
    request(&node, "PUT", "/lock/settings", &body);
    assert!(!dir.0.join("wallet.remember").exists());
}

const _: () = assert!(SPEND_COST.memory_kib >= 256 * 1024 && SPEND_COST.passes >= 3 && SPEND_COST.lanes == 1);
