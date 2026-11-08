//! `vectors.json`: every protocol v2 encoding for fixed inputs, so the app's
//! TypeScript (`apps/mobile/src/homeNode/vectors.test.ts`) can check itself
//! against the node. `OB_WRITE_VECTORS=1 cargo test -p ob-node vectors`
//! rewrites the file after a deliberate protocol change.

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Keypair, Message, Secp256k1};
use p256::ecdsa::signature::Signer as _;
use serde_json::{json, Value};
use x25519_dalek::{PublicKey as XPublic, StaticSecret};

use super::auth::Signer;
use super::pairing::{self, link};
use super::protocol::{
    base32, lan_open_request, lan_open_response, lan_seal_request, lan_seal_response_with_nonce, node_id,
    pairing_onion, pairing_onion_seed, parse_words, request_preimage, words, DeviceKey,
    PairRequest, LAN_VECTOR_RESPONSE_NONCE,
};
use crate::vault_key::encode_hex;

const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/remote/vectors.json");
const SECRET: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
const NODE_SECRET: [u8; 32] = [0x11; 32];
const P256_SECRET: [u8; 32] = [0x22; 32];
const BIP340_SECRET: [u8; 32] = [0x33; 32];
const DISCOVERY_SECRET: [u8; 32] = [0x44; 32];
const EPHEMERAL: [u8; 32] = [0x55; 32];
const WATCH_SECRET: [u8; 32] = [0x66; 32];
const TIME: u64 = 1_794_063_000;
const NONCE: &str = "00112233445566778899aabbccddeeff";

fn p256_key(secret: &[u8; 32]) -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_bytes(secret.into()).unwrap()
}

fn p256_sign(secret: &[u8; 32], preimage: &[u8]) -> String {
    let signature: p256::ecdsa::Signature = p256_key(secret).sign(preimage);
    encode_hex(signature.to_der().as_bytes())
}

fn bip340_sign(preimage: &[u8]) -> String {
    let keypair = Keypair::from_seckey_slice(&Secp256k1::new(), &BIP340_SECRET).unwrap();
    let digest = Message::from_digest(sha256::Hash::hash(preimage).to_byte_array());
    encode_hex(
        Secp256k1::new()
            .sign_schnorr_no_aux_rand(&digest, &keypair)
            .as_ref(),
    )
}

fn build() -> Value {
    let node_public = *XPublic::from(&StaticSecret::from(NODE_SECRET)).as_bytes();
    let node = node_id(&node_public);
    let p256 = DeviceKey::P256(*p256_key(&P256_SECRET).verifying_key());
    let watch = DeviceKey::P256(*p256_key(&WATCH_SECRET).verifying_key());
    let bip340 = Signer::from_secret(&BIP340_SECRET).unwrap().key();
    let discovery = tormail::discovery_public(DISCOVERY_SECRET);
    let fields = PairRequest {
        key: p256.text(),
        watch_key: Some(watch.text()),
        discovery_key: Some(discovery.clone()),
        name: "Brian's iPhone".into(),
        platform: "ios".into(),
    };
    let bare = PairRequest {
        key: bip340.text(),
        watch_key: None,
        discovery_key: None,
        name: "Desk".into(),
        platform: "desktop".into(),
    };
    let transcript = fields.transcript();
    let sas = fields.sas(&SECRET, &node_public);
    let code_words = words(&SECRET);
    let typed = code_words
        .iter()
        .enumerate()
        .map(|(i, word)| match i % 3 {
            0 => word.to_uppercase(),
            1 => word.chars().take(4).collect(),
            _ => word.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" - ");
    let body = br#"{"have":{}}"#;
    let preimage = |device: &DeviceKey| {
        request_preimage(
            &node,
            &device.id(),
            TIME,
            NONCE,
            "POST",
            "/remote/sync",
            body,
        )
    };
    let request = b"GET /remote/hello HTTP/1.1\r\nHost: lan\r\n\r\n";
    let (sealed, response_key) =
        lan_seal_request(&node_public, &StaticSecret::from(EPHEMERAL), request);
    let answer = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
    let lan_link = link(
        &SECRET,
        &node_public,
        Some(&pairing_onion(&[9; 10])),
        &["192.168.1.20:8790".to_string()],
    );
    json!({
        "comment": "Fixed inputs and the node's outputs for home node protocol v2 (docs/home-node-protocol.md). Hex unless named otherwise.",
        "node": {
            "secret": encode_hex(&NODE_SECRET),
            "public": encode_hex(&node_public),
            "nodeId": node,
        },
        "keys": {
            "p256": { "secret": encode_hex(&P256_SECRET), "text": p256.text(), "deviceId": p256.id() },
            "watch": { "secret": encode_hex(&WATCH_SECRET), "text": watch.text(), "deviceId": watch.id() },
            "bip340": { "secret": encode_hex(&BIP340_SECRET), "text": bip340.text(), "deviceId": bip340.id() },
        },
        "discovery": { "secret": encode_hex(&DISCOVERY_SECRET), "secretBase32": base32(&DISCOVERY_SECRET), "public": discovery },
        "pairing": {
            "secret": encode_hex(&SECRET),
            "secretBase32": base32(&SECRET),
            "words": code_words,
            "typed": typed,
            "onionSeed": encode_hex(&pairing_onion_seed(&SECRET)),
            "onion": pairing_onion(&SECRET),
            "link": lan_link,
        },
        "request": {
            "fields": { "key": fields.key, "watchKey": fields.watch_key, "discoveryKey": fields.discovery_key, "name": fields.name, "platform": fields.platform },
            "transcript": transcript,
            "proof": fields.proof(&SECRET),
            "signature": p256_sign(&P256_SECRET, transcript.as_bytes()),
            "sas": sas.iter().map(|(emoji, _)| *emoji).collect::<Vec<_>>(),
            "sasNames": sas.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
            "bare": { "fields": { "key": bare.key, "name": bare.name, "platform": bare.platform }, "transcript": bare.transcript(), "proof": bare.proof(&SECRET) },
        },
        "signedRequest": {
            "time": TIME,
            "nonce": NONCE,
            "method": "POST",
            "path": "/remote/sync",
            "body": String::from_utf8_lossy(body),
            "p256": { "preimage": preimage(&p256), "signature": p256_sign(&P256_SECRET, preimage(&p256).as_bytes()) },
            "bip340": { "preimage": preimage(&bip340), "signature": bip340_sign(preimage(&bip340).as_bytes()) },
        },
        "lan": {
            "ephemeralSecret": encode_hex(&EPHEMERAL),
            "ephemeralPublic": encode_hex(XPublic::from(&StaticSecret::from(EPHEMERAL)).as_bytes()),
            "request": String::from_utf8_lossy(request),
            "body": encode_hex(&sealed),
            "responseKey": encode_hex(&response_key),
            "answer": String::from_utf8_lossy(answer),
            "sealedAnswer": encode_hex(&lan_seal_response_with_nonce(&response_key, answer, &LAN_VECTOR_RESPONSE_NONCE)),
        },
    })
}

#[test]
fn vectors_match_the_file() {
    let built = build();
    if std::env::var_os("OB_WRITE_VECTORS").is_some() {
        std::fs::write(FILE, serde_json::to_string_pretty(&built).unwrap() + "\n").unwrap();
    }
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(FILE).unwrap()).unwrap();
    assert_eq!(
        stored, built,
        "vectors.json is stale; see the module comment"
    );
}

#[test]
fn vectors_check_out_in_both_directions() {
    let v = build();
    let text = |value: &Value| value.as_str().unwrap().to_string();
    let p256 = DeviceKey::parse(&text(&v["keys"]["p256"]["text"])).unwrap();
    let bip340 = DeviceKey::parse(&text(&v["keys"]["bip340"]["text"])).unwrap();
    for (key, name) in [(&p256, "p256"), (&bip340, "bip340")] {
        let signed = &v["signedRequest"][name];
        assert!(
            key.verifies(
                text(&signed["preimage"]).as_bytes(),
                &text(&signed["signature"])
            ),
            "{name}"
        );
        assert!(
            !key.verifies(b"other", &text(&signed["signature"])),
            "{name}"
        );
    }
    assert!(p256.verifies(
        text(&v["request"]["transcript"]).as_bytes(),
        &text(&v["request"]["signature"])
    ));
    assert_eq!(
        parse_words(
            &v["pairing"]["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Some(SECRET)
    );
    assert_eq!(
        parse_words(&text(&v["pairing"]["typed"])),
        Some(SECRET),
        "stems, case, and dashes"
    );
    let parsed = pairing::parse(&text(&v["pairing"]["link"])).unwrap();
    assert_eq!(parsed.secret, SECRET);
    assert_eq!(parsed.lan, ["192.168.1.20:8790"]);

    let node = StaticSecret::from(NODE_SECRET);
    let body = super::auth::decode_hex(&text(&v["lan"]["body"])).unwrap();
    let (inner, response_key) = lan_open_request(&node, &body).unwrap();
    assert_eq!(inner, text(&v["lan"]["request"]).as_bytes());
    let sealed = super::auth::decode_hex(&text(&v["lan"]["sealedAnswer"])).unwrap();
    assert_eq!(
        lan_open_response(&response_key, &sealed).unwrap(),
        text(&v["lan"]["answer"]).as_bytes()
    );
    let mut tampered = body.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert!(lan_open_request(&node, &tampered).is_none());
    assert!(
        lan_open_request(&StaticSecret::from([0x12; 32]), &body).is_none(),
        "only the node key opens it"
    );
}
