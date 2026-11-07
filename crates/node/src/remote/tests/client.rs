//! A client node (the desktop app) pairs with a home node over the LAN
//! channel on loopback and manages it.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::home_link::{exchange, Route};
use super::super::RemoteConfig;
use super::*;
use crate::harness::{hash_of, request};

#[test]
fn a_client_node_dials_only_lan_addresses_and_onions() {
    let dir = TempDir::new();
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    for lan in ["8.8.8.8:8790", "127.0.0.1:9", "example.com:80"] {
        let route = Route {
            onion: None,
            lan: &[lan.to_string()],
            node_key: Some(&[1; 32]),
        };
        assert!(
            exchange(&node, &route, b"GET / HTTP/1.1\r\n\r\n").is_err(),
            "{lan}"
        );
    }
    for code in [
        "obhome:aaaa@127.0.0.1:9",
        "openbazaar://pair?v=1&s=aaaa",
        "one two three",
    ] {
        let (status_line, value) = request(
            &node,
            "POST",
            "/home/pair",
            &json!({ "code": code, "name": "Laptop" }).to_string(),
        );
        assert!(status_line.contains("400"), "{code}: {status_line} {value}");
    }
    let (_, value) = request(&node, "GET", "/home", "");
    assert_eq!(value["paired"], false);
    assert!(request(&node, "GET", "/home/proxy/catalog", "")
        .0
        .contains("502"));
}

#[test]
fn a_paired_client_node_manages_the_home_node_over_the_lan_channel() {
    let home_dir = TempDir::new();
    let home = Node::open(&home_dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    home.set_remote(RemoteConfig {
        lan: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
        ..RemoteConfig::default()
    })
    .unwrap();
    let client_dir = TempDir::new();
    let client = Node::open(&client_dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    client
        .set_remote(RemoteConfig {
            loopback_homes: true,
            ..RemoteConfig::default()
        })
        .unwrap();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| super::super::run(&home, MAIN_PORT, &stop));
        let deadline = Instant::now() + Duration::from_secs(5);
        while home.remote.lan.addresses().is_empty() {
            assert!(Instant::now() < deadline, "the LAN channel did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        let code = home.remote.issue_code(None).unwrap();
        let link = home.remote.link_for(&code);

        let (status_line, asked) = request(
            &client,
            "POST",
            "/home/pair",
            &json!({ "code": link, "name": "Travel laptop" }).to_string(),
        );
        assert!(status_line.contains("202"), "{status_line} {asked}");
        let request_id = asked["requestId"].as_str().unwrap();
        let (_, pending) = request(&home, "GET", "/remote/pending", "");
        assert_eq!(
            pending["pending"][0]["sas"], asked["sas"],
            "both screens show the same emoji"
        );
        assert!(request(
            &home,
            "POST",
            &format!("/remote/pending/{request_id}"),
            r#"{"approve":true}"#
        )
        .0
        .contains("200"));
        let (status_line, paired) =
            request(&client, "GET", &format!("/home/pair/{request_id}"), "");
        assert!(status_line.contains("200"), "{status_line} {paired}");
        assert_eq!(paired["state"], "approved", "{paired}");
        assert_eq!(home.remote.devices().list()[0].name, "Travel laptop");
        assert_eq!(home.remote.devices().list()[0].platform, "desktop");
        let (_, again) = request(
            &client,
            "POST",
            "/home/pair",
            &json!({ "code": link, "name": "Twice" }).to_string(),
        );
        assert_eq!(again["code"], "wrong_code", "{again}");

        let (status_line, catalog) = request(&client, "GET", "/home/proxy/catalog", "");
        assert!(status_line.contains("200"), "{status_line}");
        let hash = hash_of(&home, "Paperback novel", "Marlow Goods");
        assert!(catalog["listings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|listing| listing["hash"] == hash.as_str()));
        let (_, status) = request(&client, "GET", "/home", "");
        assert_eq!(status["via"], "lan");

        let label = json!({ "target": hash, "rating": "sensitive", "tags": [] }).to_string();
        let (status_line, labeled) = request(&client, "POST", "/home/proxy/content-labels", &label);
        assert!(status_line.contains("200"), "{status_line} {labeled}");
        let new = labeled["hash"].as_str().unwrap();
        assert_eq!(
            hash_of(&home, "Paperback novel", "Marlow Goods"),
            new,
            "the home node signed the listing again"
        );
        assert_eq!(
            hash_of(&client, "Paperback novel", "Marlow Goods"),
            hash,
            "the client's own node is untouched"
        );
        let (_, devices) = request(&client, "GET", "/home/proxy/remote/devices", "");
        assert_eq!(devices["devices"][0]["thisDevice"], true);
        assert_eq!(devices["devices"][0]["lastVia"], "lan");

        let device = paired["deviceId"].as_str().unwrap();
        assert!(
            request(&home, "DELETE", &format!("/remote/devices/{device}"), "")
                .0
                .contains("200")
        );
        let (status_line, value) = request(&client, "GET", "/home/proxy/catalog", "");
        assert!(status_line.contains("401"), "{status_line} {value}");
        assert_eq!(value["code"], "unknown_device");
        assert!(request(&client, "DELETE", "/home", "").0.contains("200"));
        stop.store(true, Ordering::Relaxed);
    });
}
