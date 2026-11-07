//! Pairing: codes, proofs, emoji, approval, discovery keys.

use std::time::{Duration, Instant};

use serde_json::json;

use super::super::pairing::{Pairing, Refusal, CODE_LIFETIME};
use super::super::protocol::{decode_base32, parse_words, words};
use super::*;
use crate::harness::request;

#[test]
fn a_code_pairs_one_device_after_the_owner_approves() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    let ask = Ask::phone("Brian's iPhone");
    let (status_line, asked) = ask.send(&node, &code.secret);
    assert!(status_line.contains("202"), "{status_line} {asked}");
    let node_key = node.remote.devices().node_public;
    let expected: Vec<&str> = ask
        .fields
        .sas(&code.secret, &node_key)
        .iter()
        .map(|(emoji, _)| *emoji)
        .collect();
    assert_eq!(
        asked["sas"],
        json!(expected),
        "the device computes the same four emoji"
    );
    assert_eq!(asked["nodeKey"], encode_hex(&node_key));
    assert!(
        node.remote.devices().list().is_empty(),
        "nothing is paired before the owner approves"
    );

    let (_, pending) = request(&node, "GET", "/remote/pending", "");
    assert_eq!(pending["pending"][0]["name"], "Brian's iPhone");
    assert_eq!(
        pending["pending"][0]["sas"],
        json!(expected),
        "the node shows the same emoji"
    );
    let request_id = asked["requestId"].as_str().unwrap();
    let (status_line, decided) = request(
        &node,
        "POST",
        &format!("/remote/pending/{request_id}"),
        r#"{"approve":true}"#,
    );
    assert!(status_line.contains("200"), "{status_line} {decided}");
    let approved = status(&node, request_id);
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["nodeId"], node.remote.node_id());
    let device = Device {
        key: ask.key,
        watch: ask.watch,
        node_id: node.remote.node_id(),
    };
    let (status_line, list) = device.call(&node, "GET", "/remote/devices", "");
    assert!(status_line.contains("200"), "{status_line}");
    assert_eq!(list["devices"][0]["name"], "Brian's iPhone");
    assert_eq!(list["devices"][0]["thisDevice"], true);
    assert_eq!(list["devices"][0]["lastVia"], "loopback");
}

#[test]
fn used_wrong_and_rejected_codes_pair_nobody() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    pair_with(&node, &code.secret, Ask::phone("Phone"));
    let (status_line, value) = Ask::phone("Thief").send(&node, &code.secret);
    assert!(status_line.contains("401"), "a used code: {status_line}");
    assert_eq!(value["code"], "wrong_code");

    let (status_line, _) = Ask::phone("Guess").send(&node, &[7; SECRET_LEN]);
    assert!(status_line.contains("401"), "a guessed code: {status_line}");

    let code = node.remote.issue_code(None).unwrap();
    let (_, first) = Ask::phone("First").send(&node, &code.secret);
    let (status_line, value) = Ask::phone("Second").send(&node, &code.secret);
    assert!(
        status_line.contains("409"),
        "one request per code: {status_line}"
    );
    assert_eq!(value["code"], "code_taken");
    let request_id = first["requestId"].as_str().unwrap();
    pair_http::decide(&node, request_id, false).unwrap();
    assert_eq!(status(&node, request_id)["state"], "rejected");
    assert!(
        Ask::phone("Again")
            .send(&node, &code.secret)
            .0
            .contains("401"),
        "a rejected code is spent"
    );
    assert!(
        pair_http::decide(&node, request_id, true).is_err(),
        "a decision is final"
    );
    assert_eq!(node.remote.devices().list().len(), 1);
}

#[test]
fn a_bad_proof_or_signature_does_not_use_up_the_code() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    let ask = Ask::phone("Phone");
    let mut forged = ask.body(&code.secret);
    forged["signature"] = json!(Key::p256().sign(ask.fields.transcript().as_bytes()));
    assert!(
        send_ask(&node, &forged).0.contains("400"),
        "signed by another key"
    );
    let mut renamed = ask.body(&code.secret);
    renamed["name"] = json!("Renamed");
    assert!(
        !send_ask(&node, &renamed).0.contains("202"),
        "the proof covers the name"
    );
    let mut stolen_watch = ask.body(&code.secret);
    stolen_watch["watchKey"] = json!(Key::p256().public().text());
    assert!(
        !send_ask(&node, &stolen_watch).0.contains("202"),
        "the proof covers the watch key"
    );
    assert!(
        ask.send(&node, &code.secret).0.contains("202"),
        "the code still works"
    );
}

#[test]
fn codes_expire_and_wrong_guesses_pause_pairing() {
    let start = Instant::now();
    let mut pairing = Pairing::default();
    let code = pairing.issue(None, start);
    let ask = Ask::phone("Late");
    let late = start + CODE_LIFETIME + Duration::from_secs(1);
    assert!(pairing.live(late).is_empty());
    assert_eq!(
        pairing
            .request(ask.fields.clone(), &ask.fields.proof(&code.secret), late)
            .err(),
        Some(Refusal::Wrong),
        "expired"
    );

    let code = pairing.issue(None, late);
    for _ in 0..19 {
        assert_eq!(
            pairing
                .request(
                    ask.fields.clone(),
                    &ask.fields.proof(&[0; SECRET_LEN]),
                    late
                )
                .err(),
            Some(Refusal::Wrong)
        );
    }
    assert_eq!(
        pairing
            .request(ask.fields.clone(), &ask.fields.proof(&code.secret), late)
            .err(),
        Some(Refusal::Paused),
        "20 misses (the late one too): even the right code waits"
    );
    let later = late + Duration::from_secs(9 * 60);
    assert!(
        pairing
            .request(
                ask.fields.clone(),
                &ask.fields.proof(&code.secret),
                later + Duration::from_secs(61)
            )
            .is_err(),
        "and the code expired meanwhile"
    );
    let fresh = pairing.issue(None, later + Duration::from_secs(61));
    assert!(
        pairing
            .request(
                ask.fields.clone(),
                &ask.fields.proof(&fresh.secret),
                later + Duration::from_secs(61)
            )
            .is_ok(),
        "pairing resumes"
    );

    let mut pairing = Pairing::default();
    let codes: Vec<_> = (0..5).map(|_| pairing.issue(None, start)).collect();
    assert_eq!(pairing.live(start).len(), 4, "at most four live codes");
    assert!(
        !pairing
            .live(start)
            .iter()
            .any(|live| live.id == codes[0].id),
        "the oldest closes"
    );
}

#[test]
fn the_emoji_change_when_anyone_sits_in_the_middle() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    let ask = Ask::phone("Phone");
    let real = ask
        .fields
        .sas(&code.secret, &node.remote.devices().node_public);
    let relay_key = [0x42; 32];
    assert_ne!(
        ask.fields.sas(&code.secret, &relay_key),
        real,
        "an attacker relaying with its own node key"
    );
    let mut swapped = ask.fields.clone();
    swapped.key = Key::p256().public().text();
    assert_ne!(
        swapped.sas(&code.secret, &node.remote.devices().node_public),
        real,
        "an attacker swapping in its own device key"
    );
    let (_, asked) = ask.send(&node, &code.secret);
    let names: Vec<&str> = real.iter().map(|(_, name)| *name).collect();
    assert_eq!(asked["sasNames"], json!(names));
}

#[test]
fn approval_issues_a_discovery_key_and_removal_takes_it_back() {
    let (_dir, node) = home();
    let code = node.remote.issue_code(None).unwrap();
    let (phone, approved) = pair_with(&node, &code.secret, Ask::phone("Phone"));
    let secret: [u8; 32] = decode_base32(approved["discoverySecret"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let issued = tormail::discovery_public(secret);
    let clients = node.remote.devices().discovery_clients();
    assert_eq!(
        clients,
        [(format!("d{}", phone.id()), issued.clone())],
        "the onion admits exactly this device"
    );

    let own = tormail::discovery_public([9; 32]);
    let code = node.remote.issue_code(None).unwrap();
    let (desk, approved) = pair_with(
        &node,
        &code.secret,
        Ask::new(Key::bip340(), None, Some(own.clone()), "Desk", "desktop"),
    );
    assert!(
        approved["discoverySecret"].is_null(),
        "a device with its own key gets no secret"
    );
    assert_eq!(node.remote.devices().discovery_clients().len(), 2);
    assert!(node
        .remote
        .devices()
        .discovery_clients()
        .contains(&(format!("d{}", desk.id()), own)));

    node.remote.revoke(&phone.id()).unwrap();
    let clients = node.remote.devices().discovery_clients();
    assert_eq!(clients.len(), 1);
    assert!(
        !clients.iter().any(|(_, key)| *key == issued),
        "the removed device's key is gone"
    );

    let code = node.remote.issue_code(None).unwrap();
    let ask = Ask::new(
        Key::p256(),
        None,
        Some("descriptor:x25519:nonsense".into()),
        "Bad",
        "ios",
    );
    assert!(ask.send(&node, &code.secret).0.contains("400"));
}

#[test]
fn a_device_can_pair_another_and_the_code_closes_with_it() {
    let (_dir, node) = home();
    let laptop = first_device(&node);
    let (status_line, offer) = laptop.call(&node, "POST", "/remote/pairing", "{}");
    assert!(status_line.contains("200"), "{status_line} {offer}");
    assert_eq!(offer["words"].as_array().unwrap().len(), 8);
    assert!(offer["link"]
        .as_str()
        .unwrap()
        .starts_with("openbazaar://pair?v=2&s="));
    assert!(offer["qrSvg"].as_str().unwrap().starts_with("<?xml"));
    let secret = secret_of(&offer);
    let (status_line, asked) = Ask::phone("Phone").send(&node, &secret);
    assert!(status_line.contains("202"));
    let (_, pending) = laptop.call(&node, "GET", "/remote/pending", "");
    assert_eq!(
        pending["pending"][0]["requestId"], asked["requestId"],
        "the laptop can approve it"
    );
    node.remote.revoke(&laptop.id()).unwrap();
    assert!(
        pair_http::decide(&node, asked["requestId"].as_str().unwrap(), true).is_err(),
        "requests from a removed device's code close"
    );
    assert!(node.remote.devices().list().is_empty());
}

#[test]
fn words_carry_the_secret_with_a_checksum() {
    for _ in 0..50 {
        let secret = random_bytes::<SECRET_LEN>();
        let list = words(&secret);
        assert_eq!(parse_words(&list.join(" ")), Some(secret));
        let stems: Vec<String> = list
            .iter()
            .map(|word| word.chars().take(4).collect::<String>().to_uppercase())
            .collect();
        assert_eq!(parse_words(&stems.join("-")), Some(secret));
    }
    let list = words(&[1; SECRET_LEN]);
    let mut swapped = list.clone();
    swapped.swap(0, 1);
    assert!(
        list[0] == list[1] || parse_words(&swapped.join(" ")).is_none(),
        "a swap fails the checksum"
    );
    assert!(parse_words(&list[..7].join(" ")).is_none());
}
