//! The node's local attack surface through the router: Host and Origin
//! checks, pairing, the escrow mail drop, response headers, and malformed
//! input.

use std::time::Instant;

use proptest::prelude::*;
use serde_json::Value;
use tormail::{seal_message, SenderKey, StoreKeypair};

use crate::harness::TempDir;
use crate::http::{respond, Client};
use crate::mail_drop::MAX_PENDING;
use crate::{load_catalog, NetworkMode, Node};

const PORT: u16 = 8787;

fn node() -> (TempDir, Node) {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    (dir, node)
}

fn send(node: &Node, raw: &[u8]) -> String {
    let client = Client {
        loopback: true,
        port: PORT,
    };
    String::from_utf8(respond(node, raw, client)).unwrap()
}

fn status(response: &str) -> &str {
    response.split("\r\n").next().unwrap_or("")
}

fn body(response: &str) -> Value {
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap_or(Value::Null)
}

fn post(path: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!("POST {path} HTTP/1.1\r\n{headers}Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    raw.extend_from_slice(body);
    raw
}

fn letter() -> Vec<u8> {
    seal_message(b"escrow", StoreKeypair::generate().public(), &SenderKey::generate()).unwrap().to_bytes()
}

#[test]
fn a_rebound_host_name_is_refused_on_every_route() {
    let (_dir, node) = node();
    for host in ["evil.example:8787", "127.0.0.1.nip.io:8787", "localhost:9999"] {
        for path in ["/catalog", "/rates", "/network", "/addresses"] {
            let response = send(&node, format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes());
            assert!(status(&response).contains("421"), "{host}{path}: {}", status(&response));
            assert!(!response.contains("Marlow Goods"));
        }
    }
    let ok = send(&node, b"GET /catalog HTTP/1.1\r\nHost: localhost:8787\r\n\r\n");
    assert!(status(&ok).contains("200"));
    let missing = send(&node, b"GET /catalog HTTP/1.1\r\n\r\n");
    assert!(status(&missing).contains("421"));
}

#[test]
fn a_foreign_page_cannot_drop_escrow_mail_or_reach_any_route() {
    let (_dir, closed) = node();
    let letter = letter();
    for origin in ["https://evil.example", "null", "http://127.0.0.1:8787"] {
        let headers = format!("Host: 127.0.0.1:8787\r\nOrigin: {origin}\r\nContent-Type: text/plain\r\n");
        let response = send(&closed, &post("/escrow/mail", &headers, &letter));
        assert!(status(&response).contains("403"), "{origin}: {}", status(&response));
        let catalog = send(&closed, format!("GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: {origin}\r\n\r\n").as_bytes());
        assert!(status(&catalog).contains("403"), "{origin}");
    }
    assert!(closed.escrow.inbox.list().unwrap().is_empty());
    let shop = send(
        &closed,
        &post("/escrow/mail", "Host: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\n", &letter),
    );
    assert!(status(&shop).contains("403"), "pages never post escrow mail");

    let (shop_dir, mut serving) = node();
    let page = shop_dir.0.join("shop");
    std::fs::create_dir(&page).unwrap();
    std::fs::write(page.join("index.html"), "<html><head></head></html>").unwrap();
    serving.local.local_shop = Some(page);
    let own = send(&serving, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\n\r\n");
    assert!(status(&own).contains("200"), "the localhost shop may read the catalog");
    let own_mail = send(&serving, &post("/escrow/mail", "Host: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\n", &letter));
    assert!(status(&own_mail).contains("403"), "the shop page does not post escrow mail");
    let wallet = send(&serving, b"GET /wallet HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:8787\r\n\r\n");
    assert!(status(&wallet).contains("403"), "the shop page still needs its cookie for the wallet");
    assert!(serving.escrow.inbox.list().unwrap().is_empty());
    for origin in ["https://evil.example", "null", "http://127.0.0.1:9"] {
        let headers = format!("Host: 127.0.0.1:8787\r\nOrigin: {origin}\r\n");
        let response = send(&serving, &post("/escrow/mail", &headers, &letter));
        assert!(status(&response).contains("403"), "{origin} mail: {}", status(&response));
        let catalog = send(&serving, format!("GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: {origin}\r\n\r\n").as_bytes());
        assert!(status(&catalog).contains("403"), "{origin} catalog: {}", status(&catalog));
        let wallet = send(&serving, format!("GET /wallet HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: {origin}\r\n\r\n").as_bytes());
        assert!(status(&wallet).contains("403"), "{origin} wallet: {}", status(&wallet));
    }
    assert!(serving.escrow.inbox.list().unwrap().is_empty());

    let peer = send(&closed, &post("/escrow/mail", "Host: 127.0.0.1:8787\r\n", &letter));
    assert!(status(&peer).contains("202"), "{peer}");
    assert_eq!(closed.escrow.inbox.list().unwrap().len(), 1);
}

#[test]
fn no_request_reads_the_pairing_token_without_a_code() {
    let (_dir, node) = node();
    let token = node.pairing_token().to_string();
    for raw in [
        &b"GET /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n"[..],
        b"GET /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\n\r\n",
        b"GET /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nOrigin: tauri://localhost\r\n\r\n",
        &post("/pair", "Host: 127.0.0.1:8787\r\n", b"{}"),
        &post("/pair", "Host: 127.0.0.1:8787\r\n", br#"{"code":"AAAA-AAAA"}"#),
    ] {
        let response = send(&node, raw);
        assert!(!response.contains(&token), "{response}");
    }
}

#[test]
fn a_terminal_code_pairs_the_browser_shop_once() {
    let (_dir, node) = node();
    let mut code = String::new();
    node.pairing_codes.attempt(None, Instant::now(), |printed| code = printed.to_string());
    let request = format!(r#"{{"code":"{code}"}}"#);
    let headers = "Host: 127.0.0.1:8787\r\nOrigin: http://127.0.0.1:4173\r\nContent-Type: application/json\r\n";
    let paired = send(&node, &post("/pair", headers, request.as_bytes()));
    assert!(status(&paired).contains("200"), "{paired}");
    assert_eq!(body(&paired)["token"], node.pairing_token());
    assert!(paired.contains("Access-Control-Allow-Origin: http://127.0.0.1:4173\r\n"));
    let again = send(&node, &post("/pair", headers, request.as_bytes()));
    assert!(status(&again).contains("401"));

    let ask = send(&node, &post("/pair", headers, b"{}"));
    assert!(status(&ask).contains("429"), "a new code waits out the issue interval");
}

#[test]
fn the_desktop_node_turns_pairing_codes_off() {
    let dir = TempDir::new();
    let mut local = crate::LocalConfig::headless();
    local.pair_codes = false;
    let node = Node::open_local(
        &dir.0,
        load_catalog(NetworkMode::Testnet4).unwrap(),
        crate::NodeConfig::offline(NetworkMode::Testnet4),
        local,
    )
    .unwrap();
    let response = send(&node, &post("/pair", "Host: 127.0.0.1:8787\r\n", b"{}"));
    assert!(status(&response).contains("403"));
}

#[test]
fn the_mail_drop_caps_size_rate_and_backlog() {
    let (_dir, first) = node();
    let node = &first;
    let host = "Host: 127.0.0.1:8787\r\n";
    let oversized = send(&node, &post("/escrow/mail", host, &vec![0u8; 64 * 1024 + 1]));
    assert!(status(&oversized).contains("413"), "{}", status(&oversized));

    let mut limited = 0;
    for _ in 0..80 {
        if status(&send(&node, &post("/escrow/mail", host, b"not a letter"))).contains("429") {
            limited += 1;
        }
    }
    assert!(limited >= 10, "only {limited} of 80 rapid letters were refused");

    let (_full_dir, node) = self::node();
    for _ in 0..MAX_PENDING {
        node.escrow.inbox.accept(&tormail::SealedLetter::from_bytes(&letter()).unwrap()).unwrap();
    }
    let full = send(&node, &post("/escrow/mail", host, &letter()));
    drop(first);
    assert!(status(&full).contains("503"), "{}", status(&full));
    assert_eq!(node.escrow.inbox.list().unwrap().len(), MAX_PENDING);
}

#[test]
fn every_response_carries_the_security_headers() {
    let (_dir, node) = node();
    for raw in [
        &b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n"[..],
        b"GET /nowhere HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n",
        b"GET /catalog HTTP/1.1\r\nHost: evil.example\r\n\r\n",
        b"garbage",
    ] {
        let response = send(&node, raw);
        let head = response.split_once("\r\n\r\n").unwrap().0;
        for header in [
            "Cache-Control: no-store",
            "X-Content-Type-Options: nosniff",
            "Content-Security-Policy: default-src 'none'; frame-ancestors 'none'",
            "Referrer-Policy: no-referrer",
        ] {
            assert!(head.contains(header), "{header} missing from {head}");
        }
    }
}

#[test]
fn route_shaped_paths_do_not_panic() {
    let (_dir, node) = node();
    for path in ["/orders/pay", "/orders/release", "/orders/dispute", "/moderator/disputes/rule", "/orders/", "/addresses/"] {
        for method in ["GET", "POST", "PUT", "DELETE"] {
            let raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: 0\r\n\r\n");
            let response = send(&node, raw.as_bytes());
            assert!(response.starts_with("HTTP/1.1 "), "{method} {path}");
        }
    }
}

#[test]
fn network_warns_while_requests_go_over_clearnet() {
    let (_dir, node) = node();
    let response = send(&node, b"GET /network HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n");
    let value = body(&response);
    assert_eq!(value["clearnet"], false);
    assert!(value["privacyWarning"].as_str().unwrap().contains("not contacted"));
    assert!(value["proxy"].is_null());
    assert_eq!(value["publicChainServer"], false);
    assert!(value["chainBackend"]["backend"].is_string() && value["chainBackend"]["reveals"].is_string());
}

const PATHS: [&str; 16] = [
    "/", "/catalog", "/pair", "/rates", "/network", "/wallet", "/wallet/address", "/addresses", "/addresses/x",
    "/orders", "/orders/x/pay", "/orders/x/release", "/escrow/mail", "/escrow/sales", "/checkout", "/labelers",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn the_router_never_panics_or_leaks(
        method in prop::sample::select(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS", "PATCH"]),
        path in prop::sample::select(PATHS.to_vec()),
        suffix in "[ -~]{0,24}",
        host in prop::sample::select(vec!["127.0.0.1:8787", "localhost", "[::1]:8787", "evil.example", ""]),
        origin in prop::option::of(prop::sample::select(vec!["http://127.0.0.1:4173", "tauri://localhost", "null", "https://evil.example"])),
        token in prop::bool::ANY,
        body in prop::collection::vec(any::<u8>(), 0..512),
        noise in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        thread_local! {
            static NODE: (TempDir, Node) = node();
        }
        NODE.with(|(dir, node)| {
            let mut raw = format!("{method} {path}{suffix} HTTP/1.1\r\nHost: {host}\r\n").into_bytes();
            if let Some(origin) = origin {
                raw.extend_from_slice(format!("Origin: {origin}\r\n").as_bytes());
            }
            if token {
                raw.extend_from_slice(format!("X-Ob-Pair: {}\r\n", node.pairing_token()).as_bytes());
            }
            raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
            raw.extend_from_slice(&body);
            for raw in [raw.clone(), noise.clone(), [&raw[..raw.len().min(40)], &noise[..]].concat()] {
                let client = Client { loopback: true, port: PORT };
                let response = String::from_utf8_lossy(&respond(node, &raw, client)).into_owned();
                prop_assert!(response.starts_with("HTTP/1.1 "));
                prop_assert!(!response.contains(&dir.0.display().to_string()), "a response named the data dir");
                prop_assert!(!response.contains(node.pairing_token()), "a response carried the token");
            }
            Ok(())
        })?;
    }
}
