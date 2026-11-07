//! Remote access: pairing, signed requests, replay, revocation, the spend
//! gate, and two nodes talking over a loopback stand-in for the onion.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::auth::{pair_digest, DeviceKey, WINDOW_SECS};
use super::offers::parse_code;
use super::{gateway, now_secs, RemoteConfig};
use crate::harness::{checkout, fund_node_wallet, hash_of, open_node, request, unreachable_escrow, MockChain, TempDir, PASSPHRASE};
use crate::vault_key::{encode_hex, random_bytes};
use crate::{load_catalog, NetworkMode, Node};

/// Only ever written into Host headers; nothing listens on these here.
const MAIN_PORT: u16 = 8787;
const GATEWAY: &str = "127.0.0.1:18790";

fn split(response: Vec<u8>) -> (String, Value) {
    let text = String::from_utf8(response).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
}

fn gateway_call(node: &Node, method: &str, path: &str, headers: &str, body: &str) -> (String, Value) {
    let raw = format!("{method} {path} HTTP/1.1\r\nHost: {GATEWAY}\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len());
    split(gateway::respond(node, raw.as_bytes(), MAIN_PORT))
}

fn home() -> (TempDir, Node) {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    (dir, node)
}

struct Device {
    key: DeviceKey,
    node_id: String,
}

impl Device {
    fn pair(node: &Node, code: &str, name: &str) -> Result<Self, (String, Value)> {
        let (secret, address) = parse_code(code).unwrap();
        assert_eq!(address, GATEWAY);
        let key = DeviceKey::from_secret(&random_bytes::<32>()).unwrap();
        let public = encode_hex(&key.public().serialize());
        let body = json!({ "secret": secret, "publicKey": public, "name": name, "signature": key.sign(pair_digest(&secret, &public, name)) });
        let (status, value) = gateway_call(node, "POST", "/remote/pair", "Content-Type: application/json\r\n", &body.to_string());
        if !status.contains("200") {
            return Err((status, value));
        }
        assert_eq!(value["deviceId"], key.id());
        Ok(Self { key, node_id: value["nodeId"].as_str().unwrap().to_string() })
    }

    fn raw(&self, method: &str, path: &str, body: &str, time: u64) -> String {
        let auth = self.key.headers(&self.node_id, time, method, path, body.as_bytes());
        format!("{method} {path} HTTP/1.1\r\nHost: {GATEWAY}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    fn call(&self, node: &Node, method: &str, path: &str, body: &str) -> (String, Value) {
        split(gateway::respond(node, self.raw(method, path, body, now_secs()).as_bytes(), MAIN_PORT))
    }
}

fn first_device(node: &Node) -> Device {
    Device::pair(node, &node.remote.issue_code(None).unwrap(), "Laptop").unwrap()
}

#[test]
fn a_code_pairs_one_device_and_a_bad_proof_does_not_use_it_up() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    let (secret, _) = parse_code(&code).unwrap();
    let key = DeviceKey::from_secret(&random_bytes::<32>()).unwrap();
    let other = DeviceKey::from_secret(&random_bytes::<32>()).unwrap();
    let public = encode_hex(&key.public().serialize());
    let forged = json!({ "secret": secret, "publicKey": public, "name": "Phone", "signature": other.sign(pair_digest(&secret, &public, "Phone")) });
    let (status, _) = gateway_call(&node, "POST", "/remote/pair", "", &forged.to_string());
    assert!(status.contains("400"), "{status}");

    let phone = Device::pair(&node, &code, "Phone").unwrap();
    assert_eq!(node.remote.devices().list().len(), 1);
    let (status, value) = Device::pair(&node, &code, "Thief").err().unwrap();
    assert!(status.contains("401"), "one use: {status} {value}");
    let (status, _) = Device::pair(&node, &format!("obhome:{}@{GATEWAY}", "a".repeat(32)), "Guess").err().unwrap();
    assert!(status.contains("401"));

    let (status, catalog) = phone.call(&node, "GET", "/catalog", "");
    assert!(status.contains("200"), "{status}");
    assert!(!catalog["listings"].as_array().unwrap().is_empty());
    let (status, list) = phone.call(&node, "GET", "/remote/devices", "");
    assert!(status.contains("200"));
    assert_eq!(list["devices"][0]["name"], "Phone");
    assert_eq!(list["devices"][0]["thisDevice"], true);
}

#[test]
fn unsigned_tampered_and_unknown_requests_are_refused() {
    let (_dir, node) = home();
    let device = first_device(&node);
    for path in ["/catalog", "/orders", "/wallet", "/version", "/remote/devices"] {
        let (status, value) = gateway_call(&node, "GET", path, "", "");
        assert!(status.contains("401"), "{path}: {status}");
        assert_eq!(value["code"], "unsigned");
    }
    let (status, _) = gateway_call(&node, "POST", "/content-labels", "", "{}");
    assert!(status.contains("401"));

    let signed = device.raw("POST", "/content-labels", r#"{"target":"store","rating":"general"}"#, now_secs());
    for tampered in [
        signed.replace("/content-labels", "/content-labelz"),
        signed.replace("general", "adult\""),
        signed.replace("POST /content-labels", "PUT /content-labels"),
    ] {
        let (status, _) = split(gateway::respond(&node, tampered.as_bytes(), MAIN_PORT));
        assert!(status.contains("401") || status.contains("400"), "{status}");
    }
    let stranger = Device { key: DeviceKey::from_secret(&random_bytes::<32>()).unwrap(), node_id: device.node_id.clone() };
    let (status, value) = stranger.call(&node, "GET", "/catalog", "");
    assert!(status.contains("401"));
    assert_eq!(value["code"], "unknown_device");
    let elsewhere = Device { key: device.key, node_id: "0".repeat(32) };
    let (status, _) = elsewhere.call(&node, "GET", "/catalog", "");
    assert!(status.contains("401"), "a signature for another node: {status}");
}

#[test]
fn replays_stale_times_and_pre_restart_times_are_refused() {
    let (_dir, node) = home();
    let device = first_device(&node);
    let raw = device.raw("GET", "/orders", "", now_secs());
    assert!(split(gateway::respond(&node, raw.as_bytes(), MAIN_PORT)).0.contains("200"));
    let (status, value) = split(gateway::respond(&node, raw.as_bytes(), MAIN_PORT));
    assert!(status.contains("401"));
    assert_eq!(value["code"], "replay");

    for time in [now_secs() - WINDOW_SECS - 5, now_secs() + WINDOW_SECS + 5] {
        let (status, value) = split(gateway::respond(&node, device.raw("GET", "/orders", "", time).as_bytes(), MAIN_PORT));
        assert!(status.contains("401"));
        assert_eq!(value["code"], "clock");
        assert!(value["serverTime"].as_u64().unwrap().abs_diff(now_secs()) < 5);
    }
    let restarted = super::auth::ReplayGuard::new(now_secs() + 1);
    let earlier = device.raw("GET", "/orders", "", now_secs());
    let request = crate::http_parse::parse(earlier.as_bytes()).unwrap();
    let signed = super::auth::Signed::read(request.head).unwrap();
    assert_eq!(restarted.admit(&signed, now_secs() + 1), Err(super::auth::Refused::Clock), "a request signed before a restart");
}

#[test]
fn revoking_a_device_cuts_it_off_with_every_device_it_paired() {
    let (_dir, node) = home();
    let laptop = first_device(&node);
    let (status, minted) = laptop.call(&node, "POST", "/remote/pairing", "{}");
    assert!(status.contains("200"), "{status} {minted}");
    let phone = Device::pair(&node, minted["code"].as_str().unwrap(), "Phone").unwrap();
    let (status, minted) = phone.call(&node, "POST", "/remote/pairing", "{}");
    assert!(status.contains("200"));
    let tablet = Device::pair(&node, minted["code"].as_str().unwrap(), "Tablet").unwrap();
    let (_, waiting) = laptop.call(&node, "POST", "/remote/pairing", "{}");
    let console = first_device(&node);
    assert_eq!(node.remote.devices().list().len(), 4);

    let (status, value) = request(&node, "DELETE", &format!("/remote/devices/{}", laptop.key.id()), "");
    assert!(status.contains("200"), "{status} {value}");
    assert_eq!(value["removed"].as_array().unwrap().len(), 3, "{value}");
    for gone in [&laptop, &phone, &tablet] {
        let (status, value) = gone.call(&node, "GET", "/catalog", "");
        assert!(status.contains("401"));
        assert_eq!(value["code"], "unknown_device");
    }
    assert!(Device::pair(&node, waiting["code"].as_str().unwrap(), "Late").is_err(), "codes from a revoked device close");
    assert!(console.call(&node, "GET", "/catalog", "").0.contains("200"));
    let (status, value) = console.call(&node, "DELETE", &format!("/remote/devices/{}", console.key.id()), "");
    assert!(status.contains("200"), "a device can remove itself: {value}");
    assert!(node.remote.devices().list().is_empty());
}

#[test]
fn the_device_list_survives_a_restart() {
    let dir = TempDir::new();
    let id = {
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        node.remote.pretend_gateway(GATEWAY.parse().unwrap());
        let device = first_device(&node);
        let node_id = node.remote.node_id();
        (device, node_id)
    };
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    assert_eq!(node.remote.node_id(), id.1);
    assert_eq!(node.remote.devices().list()[0].name, "Laptop");
    assert!(id.0.call(&node, "GET", "/catalog", "").0.contains("200"));
    let sealed = std::fs::read(dir.0.join("remote-devices.obv")).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains("Laptop"), "the list is sealed");
}

#[test]
fn a_device_session_never_spends_without_the_passphrase_and_a_ticket() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, unreachable_escrow());
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    fund_node_wallet(&node, &chain, 100_000);
    let device = first_device(&node);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();

    let (status, value) = device.call(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(status.contains("400"), "no ticket: {status} {value}");
    let (status, value) = device.call(&node, "POST", &format!("/orders/{id}/pay"), r#"{"ticket":"00"}"#);
    assert!(!status.contains("20"), "a made-up ticket: {status} {value}");
    let (status, quote) = device.call(&node, "POST", &format!("/orders/{id}/pay/quote"), "");
    assert!(status.contains("200"), "{status} {quote}");
    let (status, value) = device.call(&node, "POST", "/spend-tickets", &json!({ "quote": quote["quote"] }).to_string());
    assert!(status.contains("400"), "a ticket needs the passphrase: {status} {value}");
    for (method, path, body) in [
        ("POST", "/wallet/phrase", json!({ "passphrase": PASSPHRASE })),
        ("POST", "/unlock", json!({ "remembered": true })),
        ("PUT", "/lock/settings", json!({ "passphrase": PASSPHRASE, "rememberDevice": true })),
        ("POST", "/pair", json!({})),
        ("POST", "/home/pair", json!({ "code": "x", "name": "x" })),
    ] {
        let (status, value) = device.call(&node, method, path, &body.to_string());
        assert!(status.contains("403"), "{path}: {status}");
        assert_eq!(value["code"], "node_only");
    }
    assert!(chain.broadcasts().is_empty());

    let (status, ticket) =
        device.call(&node, "POST", "/spend-tickets", &json!({ "quote": quote["quote"], "passphrase": PASSPHRASE }).to_string());
    assert!(status.contains("200"), "the passphrase and the quote make a ticket: {status} {ticket}");
    let (status, value) = device.call(&node, "POST", &format!("/orders/{id}/pay"), &json!({ "ticket": ticket["ticket"] }).to_string());
    assert!(status.contains("202"), "{status} {value}");
    assert_eq!(node.run_pay_jobs(), 1);
    assert_eq!(chain.broadcasts().len(), 1);
    let (status, _) = device.call(&node, "POST", &format!("/orders/{id}/pay"), &json!({ "ticket": ticket["ticket"] }).to_string());
    assert!(!status.contains("20"), "a ticket is spent once");
}

#[test]
fn the_gateway_answers_only_to_its_own_names_and_pages() {
    let (_dir, node) = home();
    let device = first_device(&node);
    let signed = device.raw("GET", "/catalog", "", now_secs());
    for host in ["evil.example", "127.0.0.1:8787", "localhost:1"] {
        let raw = signed.replace(&format!("Host: {GATEWAY}"), &format!("Host: {host}"));
        assert!(split(gateway::respond(&node, raw.as_bytes(), MAIN_PORT)).0.contains("421"), "{host}");
    }
    let foreign = signed.replace("\r\n\r\n", "\r\nOrigin: http://evil.onion\r\n\r\n");
    assert!(split(gateway::respond(&node, foreign.as_bytes(), MAIN_PORT)).0.contains("403"));
    let same = device.raw("GET", "/catalog", "", now_secs()).replace("\r\n\r\n", &format!("\r\nOrigin: http://{GATEWAY}\r\n\r\n"));
    assert!(split(gateway::respond(&node, same.as_bytes(), MAIN_PORT)).0.contains("200"));
    let (status, hello) = gateway_call(&node, "GET", "/remote/hello", "", "");
    assert!(status.contains("200"));
    assert_eq!(hello["nodeId"], node.remote.node_id());
    assert_eq!(hello.as_object().unwrap().len(), 2, "hello says nothing else: {hello}");
}

#[test]
fn pairing_codes_are_off_until_remote_access_is_on() {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    let (status, _) = request(&node, "POST", "/remote/pairing", "{}");
    assert!(status.contains("409"), "{status}");
    let (status, value) = request(&node, "GET", "/remote", "");
    assert!(status.contains("200"));
    assert_eq!(value["enabled"], false);
    let (status, _) = crate::http::dispatch(&node, b"GET /remote/devices HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .split(|b| *b == b'\r')
        .next()
        .map(|line| (String::from_utf8_lossy(line).to_string(), ()))
        .unwrap();
    assert!(status.contains("403"), "management needs the pairing token: {status}");
}

#[test]
fn a_client_node_refuses_home_addresses_that_are_not_onions() {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    for address in ["127.0.0.1:9", "example.com:80", "10.0.0.5:8790", "notreal.onion"] {
        let code = format!("obhome:{}@{address}", "a".repeat(32));
        let (status, value) = request(&node, "POST", "/home/pair", &json!({ "code": code, "name": "Laptop" }).to_string());
        assert!(status.contains("400"), "{address}: {status} {value}");
    }
    let (_, value) = request(&node, "GET", "/home", "");
    assert_eq!(value["paired"], false);
    let (status, _) = request(&node, "GET", "/home/proxy/catalog", "");
    assert!(status.contains("502"));
}

/// Pair a client node with a home node over loopback TCP (standing in for
/// the onion), then manage a listing on the home node through the client.
#[test]
fn a_paired_client_node_manages_the_home_node_over_the_gateway() {
    let home_dir = TempDir::new();
    let home = Node::open(&home_dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    home.set_remote(RemoteConfig { listen: Some(SocketAddr::from(([127, 0, 0, 1], 0))), ..RemoteConfig::default() }).unwrap();
    let client_dir = TempDir::new();
    let client = Node::open(&client_dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    client.set_remote(RemoteConfig { loopback_homes: true, ..RemoteConfig::default() }).unwrap();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| super::run(&home, MAIN_PORT, &stop));
        let deadline = Instant::now() + Duration::from_secs(5);
        while home.remote.address.get().is_none() {
            assert!(Instant::now() < deadline, "the gateway did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        let code = home.remote.issue_code(None).unwrap();

        let (status, paired) = request(&client, "POST", "/home/pair", &json!({ "code": code, "name": "Travel laptop" }).to_string());
        assert!(status.contains("200"), "{status} {paired}");
        let (_, again) = request(&client, "POST", "/home/pair", &json!({ "code": code, "name": "Twice" }).to_string());
        assert!(again["error"].as_str().unwrap().contains("wrong, used, or expired"), "{again}");
        assert_eq!(home.remote.devices().list()[0].name, "Travel laptop");

        let (status, catalog) = request(&client, "GET", "/home/proxy/catalog", "");
        assert!(status.contains("200"), "{status}");
        let hash = hash_of(&home, "Paperback novel", "Marlow Goods");
        assert!(catalog["listings"].as_array().unwrap().iter().any(|listing| listing["hash"] == hash.as_str()));

        let label = json!({ "target": hash, "rating": "sensitive", "tags": [] }).to_string();
        let (status, labeled) = request(&client, "POST", "/home/proxy/content-labels", &label);
        assert!(status.contains("200"), "{status} {labeled}");
        let new = labeled["hash"].as_str().unwrap();
        assert_ne!(new, hash);
        assert_eq!(hash_of(&home, "Paperback novel", "Marlow Goods"), new, "the home node signed the listing again");
        assert_eq!(hash_of(&client, "Paperback novel", "Marlow Goods"), hash, "the client's own node is untouched");

        let (status, version) = request(&client, "GET", "/home/proxy/version", "");
        assert!(status.contains("200"), "{status} {version}");
        let (status, devices) = request(&client, "GET", "/home/proxy/remote/devices", "");
        assert!(status.contains("200"));
        assert_eq!(devices["devices"][0]["thisDevice"], true);

        let device = paired["deviceId"].as_str().unwrap();
        let (status, _) = request(&home, "DELETE", &format!("/remote/devices/{device}"), "");
        assert!(status.contains("200"));
        let (status, value) = request(&client, "GET", "/home/proxy/catalog", "");
        assert!(status.contains("401"), "{status} {value}");
        assert_eq!(value["code"], "unknown_device");
        let (status, value) = request(&client, "DELETE", "/home", "");
        assert!(status.contains("200"), "{status} {value}");
        stop.store(true, Ordering::Relaxed);
    });
}
