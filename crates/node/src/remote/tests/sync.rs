//! Delta sync, batches, and the LAN channel.

use serde_json::{json, Map, Value};
use x25519_dalek::StaticSecret;

use super::super::lan;
use super::super::protocol::{lan_open_response, lan_seal_request};
use super::super::sync::{diff, etag};
use super::*;
use crate::harness::hash_of;

/// A device's replica: what it applies from each sync answer.
#[derive(Default)]
struct Replica {
    collections: Map<String, Value>,
    items: Map<String, Value>,
}

impl Replica {
    fn have(&self) -> Value {
        let mut have = Map::new();
        for (name, collection) in &self.collections {
            let items: Map<String, Value> = self
                .items
                .get(name)
                .and_then(Value::as_object)
                .map(|items| {
                    items
                        .iter()
                        .map(|(id, item)| (id.clone(), item["etag"].clone()))
                        .collect()
                })
                .unwrap_or_default();
            have.insert(
                name.clone(),
                json!({ "doc": collection["etag"], "items": items }),
            );
        }
        json!({ "have": have })
    }

    fn apply(&mut self, answer: &Value) {
        for (name, change) in answer["collections"].as_object().unwrap() {
            if change.get("error").is_some() {
                continue;
            }
            let items = self
                .items
                .entry(name.clone())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            for id in change["remove"].as_array().into_iter().flatten() {
                items.remove(id.as_str().unwrap());
            }
            for item in change["upsert"].as_array().into_iter().flatten() {
                items.insert(item["id"].as_str().unwrap().to_string(), item.clone());
            }
            if !change["doc"].is_null() {
                self.collections.insert(name.clone(), change["doc"].clone());
            }
        }
    }

    fn listing_hashes(&self) -> Vec<String> {
        let mut hashes: Vec<String> = self.items["catalog"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        hashes.sort();
        hashes
    }
}

fn changes(answer: &Value, name: &str) -> (usize, usize, bool) {
    let change = &answer["collections"][name];
    (
        change["upsert"].as_array().map_or(0, Vec::len),
        change["remove"].as_array().map_or(0, Vec::len),
        !change["doc"].is_null(),
    )
}

#[test]
fn diff_sends_only_what_changed() {
    let value =
        json!({ "total": 2, "listings": [{ "hash": "a", "n": 1 }, { "hash": "b", "n": 2 }] });
    let full = diff(value.clone(), Some(("listings", "hash")), &Value::Null);
    assert_eq!(full["upsert"].as_array().unwrap().len(), 2);
    assert_eq!(
        full["doc"]["value"],
        json!({ "total": 2 }),
        "the document travels without its items"
    );
    let have = json!({ "doc": full["doc"]["etag"], "items": { "a": etag(&json!({ "hash": "a", "n": 1 })), "b": "stale", "gone": "x" } });
    let delta = diff(value, Some(("listings", "hash")), &have);
    assert!(delta["doc"].is_null());
    assert_eq!(
        delta["upsert"],
        json!([{ "id": "b", "etag": etag(&json!({ "hash": "b", "n": 2 })), "value": { "hash": "b", "n": 2 } }])
    );
    assert_eq!(delta["remove"], json!(["gone"]));
    let doc = diff(
        json!({ "locked": true }),
        None,
        &json!({ "doc": etag(&json!({ "locked": true })) }),
    );
    assert_eq!(doc, json!({ "doc": null }));
}

#[test]
fn a_replica_follows_the_node_with_small_deltas() {
    let (_dir, node) = home();
    let phone = first_device(&node);
    let mut replica = Replica::default();
    let (status_line, first) =
        phone.call(&node, "POST", "/remote/sync", &replica.have().to_string());
    assert!(status_line.contains("200"), "{status_line} {first}");
    assert!(changes(&first, "catalog").0 > 1, "the first sync is whole");
    assert_eq!(first["node"]["nodeId"], node.remote.node_id());
    assert!(first["node"]["version"].is_object());
    replica.apply(&first);

    let (_, quiet) = phone.call(&node, "POST", "/remote/sync", &replica.have().to_string());
    for name in ["catalog", "orders", "stores", "lock", "network"] {
        assert_eq!(
            changes(&quiet, name),
            (0, 0, false),
            "{name} did not change: {}",
            quiet["collections"][name]
        );
    }

    let hash = hash_of(&node, "Paperback novel", "Marlow Goods");
    let label = json!({ "target": hash, "rating": "sensitive", "tags": [] }).to_string();
    let (status_line, labeled) = phone.call(&node, "POST", "/content-labels", &label);
    assert!(status_line.contains("200"), "{status_line} {labeled}");
    let (_, delta) = phone.call(&node, "POST", "/remote/sync", &replica.have().to_string());
    let catalog = &delta["collections"]["catalog"];
    assert_eq!(catalog["remove"], json!([hash]), "the old listing goes");
    assert_eq!(
        catalog["upsert"].as_array().unwrap().len(),
        1,
        "one new listing comes: {catalog}"
    );
    assert_eq!(catalog["upsert"][0]["id"], labeled["hash"]);
    replica.apply(&delta);

    let (_, truth) = phone.call(
        &node,
        "POST",
        "/remote/sync",
        &json!({ "have": {} }).to_string(),
    );
    let mut fresh = Replica::default();
    fresh.apply(&truth);
    assert_eq!(
        replica.listing_hashes(),
        fresh.listing_hashes(),
        "the patched replica equals a fresh one"
    );
    assert_eq!(replica.collections, fresh.collections);
}

#[test]
fn a_batch_runs_in_order_once() {
    let (_dir, node) = home();
    let phone = first_device(&node);
    let hash = hash_of(&node, "Paperback novel", "Marlow Goods");
    let id = encode_hex(&random_bytes::<16>());
    let batch = json!({ "id": id, "requests": [
        { "method": "POST", "path": "/content-labels", "body": { "target": hash, "rating": "sensitive", "tags": [] } },
        { "method": "GET", "path": "/version" },
        { "method": "POST", "path": "/remote/batch", "body": {} },
    ] });
    let (status_line, first) = phone.call(&node, "POST", "/remote/batch", &batch.to_string());
    assert!(status_line.contains("200"), "{status_line} {first}");
    assert_eq!(first["results"][0]["status"], 200, "{first}");
    assert_eq!(first["results"][1]["status"], 200);
    assert_eq!(first["results"][2]["status"], 400, "no batches in batches");
    let relabeled = hash_of(&node, "Paperback novel", "Marlow Goods");
    assert_ne!(relabeled, hash);

    let (_, again) = phone.call(&node, "POST", "/remote/batch", &batch.to_string());
    assert_eq!(again, first, "a resent batch gets the stored answer");
    assert_eq!(
        hash_of(&node, "Paperback novel", "Marlow Goods"),
        relabeled,
        "and does not run twice"
    );

    let too_many = json!({ "id": encode_hex(&random_bytes::<16>()), "requests": vec![json!({ "method": "GET", "path": "/version" }); 17] });
    assert!(phone
        .call(&node, "POST", "/remote/batch", &too_many.to_string())
        .0
        .contains("400"));
    assert!(phone
        .call(
            &node,
            "POST",
            "/remote/batch",
            r#"{"id":"short","requests":[]}"#
        )
        .0
        .contains("400"));
}

fn lan_post(node: &Node, body: &[u8]) -> Vec<u8> {
    let mut raw = format!("POST /lan HTTP/1.1\r\nHost: 192.168.1.20:8790\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    raw.extend_from_slice(body);
    lan::respond(node, &raw, MAIN_PORT)
}

fn body_of(response: &[u8]) -> &[u8] {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    &response[split + 4..]
}

#[test]
fn the_lan_channel_carries_signed_requests_sealed_to_the_node_key() {
    let (_dir, node) = home();
    let phone = first_device(&node);
    let node_key = node.remote.devices().node_public;
    let inner = phone.raw("GET", "/remote/devices", "", now_secs());
    let (body, response_key) = lan_seal_request(
        &node_key,
        &StaticSecret::from(random_bytes::<32>()),
        inner.as_bytes(),
    );
    let outer = lan_post(&node, &body);
    assert!(outer.starts_with(b"HTTP/1.1 200"));
    let answer = lan_open_response(&response_key, body_of(&outer)).expect("sealed by the node key");
    let (status_line, list) = split(answer);
    assert!(status_line.contains("200"), "{status_line}");
    assert_eq!(list["devices"][0]["lastVia"], "lan");

    assert!(
        lan_post(&node, &body).starts_with(b"HTTP/1.1 200"),
        "a replayed envelope opens…"
    );
    let replayed = lan_open_response(&response_key, body_of(&lan_post(&node, &body))).unwrap();
    assert_eq!(
        split(replayed).1["code"],
        "replay",
        "…but the signed request inside is refused"
    );

    let (wrong, _) = lan_seal_request(
        &[0x42; 32],
        &StaticSecret::from(random_bytes::<32>()),
        inner.as_bytes(),
    );
    assert!(
        lan_post(&node, &wrong).starts_with(b"HTTP/1.1 400"),
        "sealed to another key"
    );
    let mut tampered = body.clone();
    tampered[40] ^= 1;
    assert!(lan_post(&node, &tampered).starts_with(b"HTTP/1.1 400"));

    let unsigned_inner = unsigned("GET", "/catalog", "", "");
    let (body, key) = lan_seal_request(
        &node_key,
        &StaticSecret::from(random_bytes::<32>()),
        unsigned_inner.as_bytes(),
    );
    let answer = lan_open_response(&key, body_of(&lan_post(&node, &body))).unwrap();
    assert!(
        split(answer).0.contains("401"),
        "the LAN adds speed, not trust"
    );
}

#[test]
fn the_setup_page_shows_a_code_on_first_run_only() {
    let (_dir, node) = home();
    let page = String::from_utf8(lan::respond(
        &node,
        b"GET / HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("Content-Security-Policy: default-src 'none'"));
    assert!(page.contains("<svg"));
    let again = String::from_utf8(lan::respond(
        &node,
        b"GET / HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    let code_words = |page: &str| {
        page.split("<p class=words>")
            .nth(1)
            .unwrap()
            .split("</p>")
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        code_words(&page),
        code_words(&again),
        "a refresh keeps the code"
    );
    let secret = parse_words(&code_words(&page)).unwrap();
    let (_, asked) = Ask::phone("Phone").send(&node, &secret);
    let shown = String::from_utf8(lan::respond(
        &node,
        b"GET / HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    for emoji in asked["sas"].as_array().unwrap() {
        assert!(
            shown.contains(emoji.as_str().unwrap()),
            "the page shows the request's emoji"
        );
    }
    let form = format!("id={}&approve=1", asked["requestId"].as_str().unwrap());
    let get_decide = String::from_utf8(lan::respond(
        &node,
        b"GET /setup/decide HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    assert!(
        get_decide.starts_with("HTTP/1.1 303"),
        "opening the decide path returns to the setup page, {get_decide}"
    );
    assert!(
        get_decide.contains("Return to the setup page"),
        "{get_decide}"
    );
    let decide = format!("POST /setup/decide HTTP/1.1\r\nHost: openbazaar.local:8790\r\nOrigin: http://evil.example\r\nContent-Length: {}\r\n\r\n{form}", form.len());
    assert!(lan::respond(&node, decide.as_bytes(), MAIN_PORT).starts_with(b"HTTP/1.1 403"));
    assert!(node.remote.devices().list().is_empty());
    let decide = decide.replace("Origin: http://evil.example\r\n", "");
    let posted = String::from_utf8(lan::respond(&node, decide.as_bytes(), MAIN_PORT)).unwrap();
    assert!(posted.starts_with("HTTP/1.1 303"), "{posted}");
    assert!(posted.contains("Phone was approved"), "{posted}");
    assert!(posted.contains("Location: /"), "{posted}");
    assert_eq!(node.remote.devices().list().len(), 1);
    let closed = String::from_utf8(lan::respond(
        &node,
        b"GET / HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    assert!(closed.starts_with("HTTP/1.1 200"), "{closed}");
    assert!(closed.contains("Phone was approved"), "{closed}");
    assert!(
        !closed.contains("action=/setup/decide") && !closed.contains("class=words"),
        "a paired node does not offer another code or an approve form"
    );
    let again = String::from_utf8(lan::respond(
        &node,
        b"GET /setup/decide HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    assert!(again.starts_with("HTTP/1.1 303"), "{again}");
    assert!(again.contains("Phone was approved"), "{again}");
}
