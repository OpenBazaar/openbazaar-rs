//! Signed requests: signatures, replay, scope, revocation, and the spend gate.

use serde_json::json;

use super::super::auth::{Refused, ReplayGuard, Signed, WINDOW_SECS};
use super::*;
use crate::harness::{
    checkout, fund_node_wallet, open_node, request, unreachable_escrow, MockChain, PASSPHRASE,
};

#[test]
fn unsigned_tampered_and_unknown_requests_are_refused() {
    let (_dir, node) = home();
    let device = first_device(&node);
    for path in [
        "/catalog",
        "/orders",
        "/wallet",
        "/version",
        "/remote/devices",
        "/remote/sync",
    ] {
        let (status_line, value) = send(&node, Entry::Admin, &unsigned("GET", path, "", ""));
        assert!(status_line.contains("401"), "{path}: {status_line}");
        assert_eq!(value["code"], "unsigned");
    }
    assert!(send(
        &node,
        Entry::Admin,
        &unsigned("POST", "/content-labels", "", "{}")
    )
    .0
    .contains("401"));

    let signed = device.raw(
        "POST",
        "/content-labels",
        r#"{"target":"store","rating":"general"}"#,
        now_secs(),
    );
    for tampered in [
        signed.replace("/content-labels", "/content-labelz"),
        signed.replace("general", "adult\""),
        signed.replace("POST /content-labels", "PUT /content-labels"),
    ] {
        let (status_line, _) = send(&node, Entry::Admin, &tampered);
        assert!(
            status_line.contains("401") || status_line.contains("400"),
            "{status_line}"
        );
    }
    assert!(
        device.call(&node, "GET", "/catalog?q=novel", "").0.contains("200"),
        "the signature covers the query"
    );
    let added_query =
        device.raw("GET", "/catalog", "", now_secs()).replacen("/catalog", "/catalog?q=novel", 1);
    assert!(
        send(&node, Entry::Admin, &added_query).0.contains("401"),
        "a query added after signing"
    );
    let stranger = Device {
        key: Key::p256(),
        watch: None,
        node_id: device.node_id.clone(),
    };
    let (status_line, value) = stranger.call(&node, "GET", "/catalog", "");
    assert!(status_line.contains("401"));
    assert_eq!(value["code"], "unknown_device");
    let elsewhere = Device {
        key: device.key,
        watch: None,
        node_id: "0".repeat(32),
    };
    assert!(
        elsewhere
            .call(&node, "GET", "/catalog", "")
            .0
            .contains("401"),
        "a signature for another node"
    );
}

#[test]
fn replays_stale_times_and_pre_restart_times_are_refused() {
    let (_dir, node) = home();
    let device = first_device(&node);
    let raw = device.raw("GET", "/orders", "", now_secs());
    assert!(send(&node, Entry::Admin, &raw).0.contains("200"));
    let (status_line, value) = send(&node, Entry::Admin, &raw);
    assert!(status_line.contains("401"));
    assert_eq!(value["code"], "replay");
    let (status_line, value) = send(&node, Entry::Lan, &raw);
    assert!(
        status_line.contains("401"),
        "the same request over the LAN channel"
    );
    assert_eq!(value["code"], "replay");

    for time in [now_secs() - WINDOW_SECS - 5, now_secs() + WINDOW_SECS + 5] {
        let (status_line, value) =
            send(&node, Entry::Admin, &device.raw("GET", "/orders", "", time));
        assert!(status_line.contains("401"));
        assert_eq!(value["code"], "clock");
        assert!(value["serverTime"].as_u64().unwrap().abs_diff(now_secs()) < 5);
    }
    let restarted = ReplayGuard::new(now_secs() + 1);
    let earlier = device.raw("GET", "/orders", "", now_secs());
    let parsed = crate::http_parse::parse(earlier.as_bytes()).unwrap();
    let signed = Signed::read(parsed.head).unwrap();
    assert_eq!(
        restarted.admit(&signed, now_secs() + 1),
        Err(Refused::Clock),
        "a request signed before a restart"
    );
}

#[test]
fn a_watch_key_may_only_sync() {
    let (_dir, node) = home();
    let phone = first_device(&node);
    let (status_line, value) = phone.watch_call(&node, "POST", "/remote/sync", r#"{"have":{}}"#);
    assert!(status_line.contains("200"), "{status_line} {value}");
    for (method, path) in [
        ("GET", "/catalog"),
        ("GET", "/remote/devices"),
        ("POST", "/remote/batch"),
        ("PUT", "/remote/push"),
        ("POST", "/spend-tickets"),
    ] {
        let (status_line, value) = phone.watch_call(&node, method, path, "{}");
        assert!(
            status_line.contains("403"),
            "{method} {path}: {status_line}"
        );
        assert_eq!(value["code"], "watch_only");
    }
}

#[test]
fn revoking_a_device_cuts_it_off_with_every_device_it_paired() {
    let (_dir, node) = home();
    let laptop = first_device(&node);
    let (_, offer) = laptop.call(&node, "POST", "/remote/pairing", "{}");
    let (phone, _) = pair_with(&node, &secret_of(&offer), Ask::phone("Phone"));
    let (_, offer) = phone.call(&node, "POST", "/remote/pairing", "{}");
    let (tablet, _) = pair_with(&node, &secret_of(&offer), Ask::phone("Tablet"));
    let (_, waiting) = laptop.call(&node, "POST", "/remote/pairing", "{}");
    let console = first_device(&node);
    assert_eq!(node.remote.devices().list().len(), 4);

    let (status_line, value) = request(
        &node,
        "DELETE",
        &format!("/remote/devices/{}", laptop.id()),
        "",
    );
    assert!(status_line.contains("200"), "{status_line} {value}");
    assert_eq!(value["removed"].as_array().unwrap().len(), 3, "{value}");
    for gone in [&laptop, &phone, &tablet] {
        let (status_line, value) = gone.call(&node, "GET", "/catalog", "");
        assert!(status_line.contains("401"));
        assert_eq!(value["code"], "unknown_device");
        assert!(
            gone.watch_call(&node, "POST", "/remote/sync", "{}")
                .0
                .contains("401"),
            "the watch key goes too"
        );
    }
    assert!(
        Ask::phone("Late")
            .send(&node, &secret_of(&waiting))
            .0
            .contains("401"),
        "codes from a revoked device close"
    );
    assert_eq!(node.remote.devices().discovery_clients().len(), 1);
    assert!(console.call(&node, "GET", "/catalog", "").0.contains("200"));
    let (status_line, value) = console.call(
        &node,
        "DELETE",
        &format!("/remote/devices/{}", console.id()),
        "",
    );
    assert!(
        status_line.contains("200"),
        "a device can remove itself: {value}"
    );
    assert!(node.remote.devices().list().is_empty());
}

#[test]
fn the_device_list_survives_a_restart_sealed() {
    let dir = TempDir::new();
    let (device, node_id) = {
        let node = open_home(&dir);
        (first_device(&node), node.remote.node_id())
    };
    let node = open_home(&dir);
    assert_eq!(node.remote.node_id(), node_id, "the node key is kept");
    assert_eq!(node.remote.devices().list()[0].name, "Phone");
    assert!(device.call(&node, "GET", "/catalog", "").0.contains("200"));
    assert!(device
        .watch_call(&node, "POST", "/remote/sync", "{}")
        .0
        .contains("200"));
    let sealed = std::fs::read(dir.0.join("remote-devices.obv")).unwrap();
    assert!(
        !String::from_utf8_lossy(&sealed).contains("Phone"),
        "the list is sealed"
    );
}

#[test]
fn a_device_session_never_spends_without_the_passphrase_and_a_ticket() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(
        &dir.0,
        NetworkMode::Testnet4,
        &chain,
        1,
        unreachable_escrow(),
    );
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    fund_node_wallet(&node, &chain, 100_000);
    let device = first_device(&node);
    let placed = checkout(&node, &[("Paperback novel", "Marlow Goods", "direct")]);
    let id = placed[0]["orderId"].as_str().unwrap();

    let (status_line, value) = device.call(&node, "POST", &format!("/orders/{id}/pay"), "{}");
    assert!(
        status_line.contains("400"),
        "no ticket: {status_line} {value}"
    );
    let (status_line, value) = device.call(
        &node,
        "POST",
        &format!("/orders/{id}/pay"),
        r#"{"ticket":"00"}"#,
    );
    assert!(
        !status_line.contains("20"),
        "a made-up ticket: {status_line} {value}"
    );
    let (status_line, quote) = device.call(&node, "POST", &format!("/orders/{id}/pay/quote"), "");
    assert!(status_line.contains("200"), "{status_line} {quote}");
    let (status_line, value) = device.call(
        &node,
        "POST",
        "/spend-tickets",
        &json!({ "quote": quote["quote"] }).to_string(),
    );
    assert!(
        status_line.contains("400"),
        "a ticket needs the passphrase: {status_line} {value}"
    );
    // Until the backup is confirmed, a device can read the phrase again:
    // it may be the only screen the node has. After that, never.
    let phrase = json!({ "passphrase": PASSPHRASE }).to_string();
    let (status_line, value) = device.call(&node, "POST", "/wallet/phrase", &phrase);
    assert!(status_line.contains("200") && value["phrase"].is_string(), "{status_line}");
    node.lock.mark_backup_confirmed().unwrap();
    for (method, path, body) in [
        (
            "POST",
            "/wallet/phrase",
            json!({ "passphrase": PASSPHRASE }),
        ),
        ("POST", "/unlock", json!({ "remembered": true })),
        (
            "PUT",
            "/lock/settings",
            json!({ "passphrase": PASSPHRASE, "rememberDevice": true }),
        ),
        ("POST", "/pair", json!({})),
        ("POST", "/home/pair", json!({ "code": "x", "name": "x" })),
        ("POST", "/remote/lan-page", json!({})),
    ] {
        let (status_line, value) = device.call(&node, method, path, &body.to_string());
        assert!(status_line.contains("403"), "{path}: {status_line}");
        assert_eq!(value["code"], "node_only");
    }
    let batch = json!({ "id": "a".repeat(32), "requests": [{ "method": "POST", "path": "/wallet/phrase", "body": { "passphrase": PASSPHRASE } }] });
    let (_, value) = device.call(&node, "POST", "/remote/batch", &batch.to_string());
    assert_eq!(
        value["results"][0]["status"], 403,
        "a batch goes through the same policy: {value}"
    );
    assert!(chain.broadcasts().is_empty());

    let (status_line, ticket) = device.call(
        &node,
        "POST",
        "/spend-tickets",
        &json!({ "quote": quote["quote"], "passphrase": PASSPHRASE }).to_string(),
    );
    assert!(
        status_line.contains("200"),
        "the passphrase and the quote make a ticket: {status_line} {ticket}"
    );
    let (status_line, value) = device.call(
        &node,
        "POST",
        &format!("/orders/{id}/pay"),
        &json!({ "ticket": ticket["ticket"] }).to_string(),
    );
    assert!(status_line.contains("202"), "{status_line} {value}");
    assert_eq!(node.run_pay_jobs(), 1);
    assert_eq!(chain.broadcasts().len(), 1);
    let (status_line, _) = device.call(
        &node,
        "POST",
        &format!("/orders/{id}/pay"),
        &json!({ "ticket": ticket["ticket"] }).to_string(),
    );
    assert!(!status_line.contains("20"), "a ticket is spent once");
}

#[test]
fn the_gateway_answers_only_to_its_own_names_and_pages() {
    let (_dir, node) = home();
    let device = first_device(&node);
    let signed = device.raw("GET", "/catalog", "", now_secs());
    for host in ["evil.example", "127.0.0.1:8787", "localhost:1"] {
        let raw = signed.replace(&format!("Host: {GATEWAY}"), &format!("Host: {host}"));
        assert!(send(&node, Entry::Admin, &raw).0.contains("421"), "{host}");
    }
    let foreign = signed.replace("\r\n\r\n", "\r\nOrigin: http://evil.onion\r\n\r\n");
    assert!(send(&node, Entry::Admin, &foreign).0.contains("403"));
    let same = device
        .raw("GET", "/catalog", "", now_secs())
        .replace("\r\n\r\n", &format!("\r\nOrigin: http://{GATEWAY}\r\n\r\n"));
    assert!(send(&node, Entry::Admin, &same).0.contains("200"));
    let (status_line, hello) = send(
        &node,
        Entry::Admin,
        &unsigned("GET", "/remote/hello", "", ""),
    );
    assert!(status_line.contains("200"));
    assert_eq!(hello["nodeId"], node.remote.node_id());
    assert_eq!(
        hello.as_object().unwrap().len(),
        2,
        "hello says nothing else: {hello}"
    );
    let (status_line, _) = send(
        &node,
        Entry::Pairing,
        &device.raw("GET", "/catalog", "", now_secs()),
    );
    assert!(
        status_line.contains("404"),
        "the pairing onion only pairs: {status_line}"
    );
}

#[test]
fn pairing_codes_are_off_until_remote_access_is_on() {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    let (status_line, _) = request(&node, "POST", "/remote/pairing", "{}");
    assert!(status_line.contains("409"), "{status_line}");
    let (_, value) = request(&node, "GET", "/remote", "");
    assert_eq!(value["enabled"], false);
    let answer = crate::http::dispatch(
        &node,
        b"GET /remote/devices HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
    );
    assert!(
        String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 403"),
        "management needs the pairing token"
    );
    let body = json!({ "v": 2 });
    assert!(
        send_ask(&node, &body).0.contains("409"),
        "no pairing while remote access is off"
    );
}

#[test]
fn push_is_off_until_a_device_turns_it_on_for_itself() {
    let (_dir, node) = home();
    let phone = first_device(&node);
    let (_, list) = phone.call(&node, "GET", "/remote/devices", "");
    assert_eq!(list["devices"][0]["push"]["enabled"], false);
    for bad in [
        json!({ "platform": "sms", "token": "abc" }),
        json!({ "platform": "apns", "token": "" }),
        json!({ "platform": "fcm", "token": "a b" }),
    ] {
        assert!(
            phone
                .call(&node, "PUT", "/remote/push", &bad.to_string())
                .0
                .contains("400"),
            "{bad}"
        );
    }
    let (status_line, list) = phone.call(
        &node,
        "PUT",
        "/remote/push",
        r#"{"platform":"apns","token":"a1b2c3"}"#,
    );
    assert!(status_line.contains("200"), "{status_line} {list}");
    assert_eq!(
        list["devices"][0]["push"],
        json!({ "enabled": true, "platform": "apns" }),
        "the token itself is not shown"
    );
    assert!(!list.to_string().contains("a1b2c3"));
    let (_, list) = phone.call(&node, "DELETE", "/remote/push", "");
    assert_eq!(list["devices"][0]["push"]["enabled"], false);
    assert!(
        request(
            &node,
            "PUT",
            "/remote/push",
            r#"{"platform":"apns","token":"x"}"#
        )
        .0
        .contains("400"),
        "the node has no push token of its own"
    );

    assert!(super::super::push::gateway_target("http://example.com").is_err());
    assert!(
        super::super::push::gateway_target(&format!("https://{}.onion", "a".repeat(56))).is_err()
    );
}
