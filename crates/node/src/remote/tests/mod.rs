//! Remote access: pairing, signed requests, replay, revocation, the spend
//! gate, sync and batches, the LAN channel, and two nodes talking over it.

mod client;
mod moderators;
mod pairing;
mod passkey;
mod requests;
mod sync;
mod web;

use p256::ecdsa::signature::Signer as _;
use serde_json::{json, Value};

use super::auth::Signer;
use super::gateway::{self, Entry};
use super::protocol::{parse_words, DeviceKey, PairRequest, SECRET_LEN};
use super::{now_secs, pair_http};
use crate::harness::TempDir;
use crate::vault_key::{encode_hex, random_bytes};
use crate::{load_catalog, NetworkMode, Node};

/// Only ever written into Host headers; nothing listens on these here.
const MAIN_PORT: u16 = 8787;
const GATEWAY: &str = "127.0.0.1:18790";

fn split(response: Vec<u8>) -> (String, Value) {
    let text = String::from_utf8(response).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    (
        head.lines().next().unwrap().to_string(),
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}

fn send(node: &Node, entry: Entry, raw: &str) -> (String, Value) {
    split(gateway::respond(node, raw.as_bytes(), entry, MAIN_PORT))
}

fn unsigned(method: &str, path: &str, headers: &str, body: &str) -> String {
    format!(
        "{method} {path} HTTP/1.1\r\nHost: {GATEWAY}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn open_home(dir: &TempDir) -> Node {
    let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    node.remote.pretend_gateway(GATEWAY.parse().unwrap());
    node
}

fn home() -> (TempDir, Node) {
    let dir = TempDir::new();
    let node = open_home(&dir);
    (dir, node)
}

/// A device key as the apps hold them: P-256 for native apps, BIP 340 for
/// browsers and client nodes.
enum Key {
    P256(p256::ecdsa::SigningKey),
    Bip340(Signer),
}

impl Key {
    fn p256() -> Self {
        Self::P256(p256::ecdsa::SigningKey::random(
            &mut p256::elliptic_curve::rand_core::OsRng,
        ))
    }

    fn bip340() -> Self {
        Self::Bip340(Signer::from_secret(&random_bytes::<32>()).unwrap())
    }

    fn public(&self) -> DeviceKey {
        match self {
            Self::P256(key) => DeviceKey::P256(*key.verifying_key()),
            Self::Bip340(signer) => signer.key(),
        }
    }

    fn sign(&self, preimage: &[u8]) -> String {
        match self {
            Self::P256(key) => {
                let signature: p256::ecdsa::Signature = key.sign(preimage);
                encode_hex(signature.to_der().as_bytes())
            }
            Self::Bip340(signer) => signer.sign(preimage),
        }
    }
}

struct Device {
    key: Key,
    watch: Option<Key>,
    node_id: String,
}

impl Device {
    fn id(&self) -> String {
        self.key.public().id()
    }

    fn raw_as(&self, key: &Key, method: &str, path: &str, body: &str, time: u64) -> String {
        let id = key.public().id();
        let nonce = encode_hex(&random_bytes::<16>());
        let preimage = super::protocol::request_preimage(
            &self.node_id,
            &id,
            time,
            &nonce,
            method,
            path,
            body.as_bytes(),
        );
        let signature = key.sign(preimage.as_bytes());
        unsigned(
            method,
            path,
            &format!("X-Ob-Device: {id}\r\nX-Ob-Time: {time}\r\nX-Ob-Nonce: {nonce}\r\nX-Ob-Signature: {signature}\r\nContent-Type: application/json\r\n"),
            body,
        )
    }

    fn raw(&self, method: &str, path: &str, body: &str, time: u64) -> String {
        self.raw_as(&self.key, method, path, body, time)
    }

    fn call(&self, node: &Node, method: &str, path: &str, body: &str) -> (String, Value) {
        send(
            node,
            Entry::Admin,
            &self.raw(method, path, body, now_secs()),
        )
    }

    fn watch_call(&self, node: &Node, method: &str, path: &str, body: &str) -> (String, Value) {
        send(
            node,
            Entry::Admin,
            &self.raw_as(self.watch.as_ref().unwrap(), method, path, body, now_secs()),
        )
    }
}

/// What a device sends to `POST /pair/request`.
struct Ask {
    key: Key,
    watch: Option<Key>,
    fields: PairRequest,
}

impl Ask {
    fn new(
        key: Key,
        watch: Option<Key>,
        discovery: Option<String>,
        name: &str,
        platform: &str,
    ) -> Self {
        let fields = PairRequest {
            key: key.public().text(),
            watch_key: watch.as_ref().map(|watch| watch.public().text()),
            discovery_key: discovery,
            name: name.into(),
            platform: platform.into(),
        };
        Self { key, watch, fields }
    }

    fn phone(name: &str) -> Self {
        Self::new(Key::p256(), Some(Key::p256()), None, name, "ios")
    }

    fn body(&self, secret: &[u8; SECRET_LEN]) -> Value {
        json!({
            "v": 2,
            "key": self.fields.key,
            "watchKey": self.fields.watch_key,
            "discoveryKey": self.fields.discovery_key,
            "name": self.fields.name,
            "platform": self.fields.platform,
            "proof": self.fields.proof(secret),
            "signature": self.key.sign(self.fields.transcript().as_bytes()),
        })
    }

    fn send(&self, node: &Node, secret: &[u8; SECRET_LEN]) -> (String, Value) {
        send_ask(node, &self.body(secret))
    }
}

fn send_ask(node: &Node, body: &Value) -> (String, Value) {
    let body = body.to_string();
    send(
        node,
        Entry::Pairing,
        &unsigned(
            "POST",
            "/pair/request",
            "Content-Type: application/json\r\n",
            &body,
        ),
    )
}

fn status(node: &Node, request_id: &str) -> Value {
    send(
        node,
        Entry::Pairing,
        &unsigned("GET", &format!("/pair/status/{request_id}"), "", ""),
    )
    .1
}

/// Ask, approve on the node, and read the approval like a device would.
fn pair_with(node: &Node, secret: &[u8; SECRET_LEN], ask: Ask) -> (Device, Value) {
    let (status_line, asked) = ask.send(node, secret);
    assert!(status_line.contains("202"), "{status_line} {asked}");
    let request_id = asked["requestId"].as_str().unwrap();
    pair_http::decide(node, request_id, true).unwrap();
    let approved = status(node, request_id);
    assert_eq!(approved["state"], "approved", "{approved}");
    assert_eq!(approved["deviceId"], ask.key.public().id());
    let device = Device {
        key: ask.key,
        watch: ask.watch,
        node_id: approved["nodeId"].as_str().unwrap().to_string(),
    };
    (device, approved)
}

fn first_device(node: &Node) -> Device {
    let code = node.remote.issue_code(None).unwrap();
    pair_with(node, &code.secret, Ask::phone("Phone")).0
}

/// The secret of a code a device asked for over `POST /remote/pairing`.
fn secret_of(offer: &Value) -> [u8; SECRET_LEN] {
    let words: Vec<&str> = offer["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap())
        .collect();
    parse_words(&words.join(" ")).unwrap()
}
