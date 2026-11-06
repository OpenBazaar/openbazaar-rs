use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;

use openbazaar_protocol::{fixture_catalog, ContentLabel, ContentRating, ContentTags, LabelEntry, LabelTarget};

use super::*;
use crate::{load_catalog, NetworkMode};

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("ob-labelers-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn open(dir: &TempDir) -> Labelers {
    Labelers::open(&dir.0, Vault::open(&dir.0).unwrap()).unwrap()
}

/// Serve `body` once per connection, `count` times, on a free loopback port.
fn serve(body: Vec<u8>, count: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(count) {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    format!("http://127.0.0.1:{port}/labels")
}

fn titles(value: &Value) -> Vec<String> {
    let catalog = fixture_catalog();
    let mut out: Vec<String> = value["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| {
            let hash = label["target"].as_str().unwrap();
            let listing = catalog.listings.iter().find(|listing| listing.content_hash().unwrap().to_hex() == hash).unwrap();
            format!("{}:{}:{}", listing.title, label["rating"].as_str().unwrap(), label["reason"].as_str().unwrap())
        })
        .collect();
    out.sort();
    out
}

fn other_set(sequence: u64) -> LabelSet {
    let listing = &fixture_catalog().listings[0];
    LabelSet::sign(
        "Local labeler",
        sequence,
        20261106,
        vec![LabelEntry {
            target: LabelTarget::Listing(listing.content_hash().unwrap()),
            label: Some(ContentLabel::new(ContentRating::Adult, ContentTags::NUDITY)),
            reason: "explicit".to_string(),
        }],
        &bitcoin::secp256k1::SecretKey::from_slice(&[7u8; 32]).unwrap(),
    )
    .unwrap()
}

#[test]
fn follows_nobody_by_default_and_offers_the_sample() {
    let dir = temp_dir("default");
    let value = open(&dir).current();
    assert_eq!(value["labelers"], json!([]));
    assert_eq!(value["labels"], json!([]));
    assert_eq!(value["samples"][0]["key"], hex(&sample_key()));
}

#[test]
fn following_the_sample_serves_its_current_labels_and_unfollowing_clears_them() {
    let dir = temp_dir("sample");
    let mut labelers = open(&dir);
    let key = hex(&sample_key());
    let value = labelers.replace(&json!({ "labelers": [{ "key": key }] })).unwrap();
    assert_eq!(
        titles(&value),
        [
            "Kitchen shears:sensitive:blade",
            "Sewing shears:sensitive:blade",
            "Whole bean coffee, 12 oz:sensitive:stimulant",
        ]
    );
    assert_eq!(value["labelers"][0]["name"], "Sample labeler");
    assert_eq!(value["labelers"][0]["sequence"], 2);
    assert_eq!(value["labels"][0]["labelerName"], "Sample labeler");
    assert_eq!(value["labels"][0]["tags"].as_array().unwrap().len(), 1);

    let sealed = fs::read(dir.0.join(FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains(&key), "the follow list is sealed");
    assert_eq!(titles(&open(&dir).current()).len(), 3, "the follow list survives a restart");

    let value = labelers.replace(&json!({ "labelers": [] })).unwrap();
    assert_eq!(value["labels"], json!([]));
    assert_eq!(open(&dir).current()["labelers"], json!([]));
}

#[test]
fn fetches_a_local_labeler_url_and_keeps_only_sets_signed_by_the_followed_key() {
    let dir = temp_dir("url");
    let mut labelers = open(&dir);
    let (first, second) = (other_set(1), other_set(2));
    let mut bytes = first.encode().unwrap();
    bytes.extend(second.encode().unwrap());
    let url = serve(bytes.clone(), 1);
    let key = first.labeler_hex();
    let value = labelers.replace(&json!({ "labelers": [{ "key": key, "url": url }] })).unwrap();
    assert_eq!(value["labelers"][0]["error"], Value::Null, "{value}");
    assert_eq!(value["labelers"][0]["sequence"], 2);
    assert_eq!(titles(&value), ["Paperback novel:adult:explicit"]);

    let wrong_key = hex(&sample_key());
    let url = serve(bytes.clone(), 1);
    let value = labelers.replace(&json!({ "labelers": [{ "key": wrong_key, "url": url }] })).unwrap();
    assert!(value["labelers"][0]["error"].as_str().unwrap().contains("different key"));
    assert_eq!(value["labels"], json!([]));

    let mut tampered = bytes;
    let rating = first.encode().unwrap().len() - 64 - 2 - "explicit".len() - 2 - 1;
    tampered[rating] = 0;
    let url = serve(tampered, 1);
    let value = labelers.replace(&json!({ "labelers": [{ "key": key, "url": url }] })).unwrap();
    assert!(value["labelers"][0]["error"].as_str().unwrap().contains("did not verify"), "{value}");
    assert_eq!(value["labels"], json!([]));
}

#[test]
fn refuses_keys_and_urls_that_are_not_local() {
    let dir = temp_dir("refuse");
    let mut labelers = open(&dir);
    let key = hex(&sample_key());
    for url in [
        "https://labels.example",
        "http://labels.example/sets",
        "http://127.0.0.1@labels.example/",
        "http://127.0.0.1.labels.example:80/",
        "http://10.0.0.2:9000/",
        "file:///etc/passwd",
    ] {
        assert!(labelers.replace(&json!({ "labelers": [{ "key": key, "url": url }] })).is_err(), "{url}");
    }
    for url in ["http://127.0.0.1:9000/labels", "http://localhost/labels", "http://[::1]:9000/"] {
        assert!(is_local_http(url), "{url}");
    }
    for key in ["abc", &"z".repeat(64), &"0".repeat(64)] {
        assert!(labelers.replace(&json!({ "labelers": [{ "key": key }] })).is_err(), "{key}");
    }
    let unknown = other_set(1).labeler_hex();
    let value = labelers.replace(&json!({ "labelers": [{ "key": unknown }] })).unwrap();
    assert!(value["labelers"][0]["error"].as_str().unwrap().contains("No address"));
    assert_eq!(value["labels"], json!([]));
}

#[test]
fn endpoint_is_paired_loopback_only() {
    let dir = temp_dir("http");
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    let body = json!({ "labelers": [{ "key": hex(&sample_key()) }] }).to_string();
    let put = |token: &str| {
        format!("PUT /labelers HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nX-Ob-Pair: {token}\r\n\r\n{body}", body.len()).into_bytes()
    };
    let status = |response: Vec<u8>| String::from_utf8(response).unwrap().lines().next().unwrap().to_string();

    assert!(status(crate::http::dispatch(&node, &put("nope"))).contains("403"));
    let stranger = Client { loopback: false, port: 8787 };
    assert!(status(crate::http::respond(&node, &put(node.pairing_token()), stranger)).contains("403"));
    assert!(status(crate::http::dispatch(&node, b"GET /labelers HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")).contains("403"));

    let ok = String::from_utf8(crate::http::dispatch(&node, &put(node.pairing_token()))).unwrap();
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let get = format!("GET /labelers HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Ob-Pair: {}\r\n\r\n", node.pairing_token());
    let text = String::from_utf8(crate::http::dispatch(&node, get.as_bytes())).unwrap();
    let value: Value = serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(value["labels"].as_array().unwrap().len(), 3);
}
