//! Pairing: codes, proofs, emoji, approval, discovery keys.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::lan;
use super::super::pair_ready::{Onion, SLOW_AFTER};
use super::super::pairing::{Pairing, Refusal, CODE_LIFETIME};
use super::super::protocol::{base32, decode_base32, pairing_onion, parse_words, words};
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
fn a_second_pending_attempt_from_the_same_device_replaces_the_first() {
    let (_dir, node) = home();
    let fresh = setup_page(&node);
    assert!(
        !fresh.contains("wants to pair"),
        "with nothing waiting, the page is the code alone"
    );
    assert!(fresh
        .contains("One use. It expires in 10 minutes. This page is only on your home network."));
    assert!(fresh.contains("<style"));
    assert!(!fresh.contains("<script"));

    let phone = Ask::phone("iPhone");
    let first_secret = parse_words(&words_on(&fresh)).unwrap();
    let (_, first) = phone.send(&node, &first_secret);
    let first_id = first["requestId"].as_str().unwrap().to_string();

    let rotated = setup_page(&node);
    let second_secret = parse_words(&words_on(&rotated)).unwrap();
    assert_ne!(first_secret, second_secret, "a taken code leaves the page");
    let (_, second) = phone.send(&node, &second_secret);
    let second_id = second["requestId"].as_str().unwrap().to_string();
    assert_ne!(second_id, first_id);

    let (_, pending) = request(&node, "GET", "/remote/pending", "");
    assert_eq!(
        pending_ids(&pending),
        vec![second_id.as_str()],
        "the second attempt replaces the first"
    );
    assert_eq!(
        status(&node, &first_id)["state"],
        "expired",
        "the replaced attempt is gone"
    );

    let shown = setup_page(&node);
    assert_eq!(shown.matches("wants to pair").count(), 1);
    assert!(shown.contains(&format!("value={second_id}")));
    assert!(!shown.contains(&format!("value={first_id}")));
    assert!(shown.contains(">Approve<") && shown.contains(">Reject<"));

    let (_, by_name) =
        Ask::phone("iPhone").send(&node, &node.remote.issue_code(None).unwrap().secret);
    let name_id = by_name["requestId"].as_str().unwrap().to_string();
    let (_, pending) = request(&node, "GET", "/remote/pending", "");
    assert_eq!(
        pending_ids(&pending),
        vec![name_id.as_str()],
        "the same device name replaces, even with a new key"
    );

    let pixel = Ask::new(Key::p256(), Some(Key::p256()), None, "Pixel", "android");
    let (_, other) = pixel.send(&node, &node.remote.issue_code(None).unwrap().secret);
    let other_id = other["requestId"].as_str().unwrap().to_string();
    let both = setup_page(&node);
    assert_eq!(
        both.matches("wants to pair").count(),
        2,
        "two devices can wait"
    );
    assert!(both.contains(&format!("value={name_id}")));
    assert!(both.contains(&format!("value={other_id}")));

    pair_http::decide(&node, &other_id, false).unwrap();
    let after = setup_page(&node);
    assert_eq!(status(&node, &other_id)["state"], "rejected");
    assert_eq!(after.matches("wants to pair").count(), 1);
    assert!(
        !after.contains(&format!("value={other_id}")),
        "a rejected attempt disappears"
    );
    assert!(after.contains(&format!("value={name_id}")));
    assert!(after
        .contains("One use. It expires in 10 minutes. This page is only on your home network."));

    let mut pairing = Pairing::default();
    let now = Instant::now();
    let ask = Ask::phone("iPhone");
    let code = pairing.issue(None, now);
    let first = pairing
        .request(ask.fields.clone(), &ask.fields.proof(&code.secret), now)
        .unwrap();
    let mut renamed = ask.fields.clone();
    renamed.name = "Kitchen iPhone".into();
    let code = pairing.issue(None, now);
    let second = pairing
        .request(renamed.clone(), &renamed.proof(&code.secret), now)
        .unwrap();
    assert_eq!(
        pairing.pending(now).len(),
        1,
        "the same device key replaces"
    );
    assert_eq!(pairing.pending(now)[0].0.id, second.id);
    assert!(pairing.get(&first.id, now).is_none());

    let code = pairing.issue(None, now);
    let late = Ask::phone("Late");
    pairing
        .request(late.fields.clone(), &late.fields.proof(&code.secret), now)
        .unwrap();
    assert_eq!(
        pairing.pending(now).len(),
        2,
        "a different device stays pending"
    );
    assert!(
        pairing
            .pending(now + CODE_LIFETIME + Duration::from_secs(1))
            .is_empty(),
        "an expired attempt disappears"
    );
}

fn setup_page(node: &Node) -> String {
    let page = String::from_utf8(lan::respond(
        node,
        b"GET / HTTP/1.1\r\nHost: openbazaar.local:8790\r\n\r\n",
        MAIN_PORT,
    ))
    .unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    page
}

fn words_on(page: &str) -> String {
    page.split("<p class=words>")
        .nth(1)
        .unwrap()
        .split("</p>")
        .next()
        .unwrap()
        .to_string()
}

fn pending_ids(pending: &serde_json::Value) -> Vec<&str> {
    pending["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["requestId"].as_str().unwrap())
        .collect()
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

#[test]
fn a_code_waits_for_its_onion_without_spending_its_ten_minutes() {
    let start = Instant::now();
    let secs = Duration::from_secs;
    let mut pairing = Pairing::default();
    let onion_at = |pairing: &mut Pairing, id: &str, at: Instant| {
        let code = pairing.live(at).into_iter().find(|code| code.id == id)?;
        Some((code.onion(at), code.expires_in(at)))
    };

    // Without Tor there is no onion to wait for.
    let plain = pairing.issue(None, start);
    assert_eq!(onion_at(&mut pairing, &plain.id, start), Some((None, CODE_LIFETIME)));

    // The node reaches the onion after 40 s: the ten minutes start then.
    let reached = Arc::new(AtomicBool::new(false));
    let id = pairing.issue(None, start).id;
    let code = pairing.watch_onion(&id, Arc::clone(&reached)).unwrap();
    assert_eq!(code.onion(start), Some(Onion::Publishing));
    assert_eq!(
        onion_at(&mut pairing, &id, start + secs(39)).unwrap().0,
        Some(Onion::Publishing)
    );
    reached.store(true, Ordering::Relaxed);
    let ready = start + secs(40);
    assert_eq!(onion_at(&mut pairing, &id, ready), Some((Some(Onion::Ready), CODE_LIFETIME)));
    assert!(onion_at(&mut pairing, &id, ready + CODE_LIFETIME - secs(1)).is_some());
    assert!(onion_at(&mut pairing, &id, ready + CODE_LIFETIME).is_none(), "and end ten minutes later");

    // The node never reaches it: after the limit the code is shown anyway,
    // with its full ten minutes, and a late answer does not start them again.
    let reached = Arc::new(AtomicBool::new(false));
    let id = pairing.issue(None, start).id;
    pairing.watch_onion(&id, Arc::clone(&reached)).unwrap();
    let slow = start + SLOW_AFTER;
    assert_eq!(
        onion_at(&mut pairing, &id, slow - secs(1)).unwrap().0,
        Some(Onion::Publishing)
    );
    assert_eq!(onion_at(&mut pairing, &id, slow), Some((Some(Onion::Slow), CODE_LIFETIME)));
    reached.store(true, Ordering::Relaxed);
    assert_eq!(
        onion_at(&mut pairing, &id, slow + secs(60)),
        Some((Some(Onion::Ready), CODE_LIFETIME - secs(60)))
    );

    // Nobody asks about a code for five minutes (a code made with `ob-node
    // pair`): its ten minutes still began no later than the limit.
    let id = pairing.issue(None, start).id;
    pairing.watch_onion(&id, Arc::new(AtomicBool::new(true))).unwrap();
    assert_eq!(
        onion_at(&mut pairing, &id, start + secs(300)),
        Some((Some(Onion::Ready), CODE_LIFETIME - (secs(300) - SLOW_AFTER)))
    );

    // A device that asks while the page still holds the code is not turned away.
    let reached = Arc::new(AtomicBool::new(false));
    let code = pairing.issue(None, start);
    pairing.watch_onion(&code.id, reached).unwrap();
    let ask = Ask::phone("Early");
    assert!(pairing
        .request(ask.fields.clone(), &ask.fields.proof(&code.secret), start + secs(5))
        .is_ok());
}

#[test]
fn the_page_hears_how_a_codes_onion_is_doing() {
    let (_dir, node) = home();
    let (status_line, offer) = request(&node, "POST", "/remote/pairing", "{}");
    assert!(status_line.contains("200"), "{status_line} {offer}");
    assert!(offer["onion"].is_null(), "no Tor in this test, so nothing to wait for");
    assert_eq!(offer["lan"], false);
    let id = offer["codeId"].as_str().unwrap();
    let codes = || request(&node, "GET", "/remote/pending", "").1["codes"].clone();
    assert_eq!(codes()[0]["codeId"], id);
    assert!(codes()[0]["onion"].is_null());

    // As when Tor is up and the code has an onion.
    let reached = Arc::new(AtomicBool::new(false));
    node.remote.pairing().watch_onion(id, Arc::clone(&reached)).unwrap();
    assert_eq!(codes()[0]["onion"], "publishing");
    reached.store(true, Ordering::Relaxed);
    assert_eq!(codes(), json!([{ "codeId": id, "onion": "ready", "expiresInSecs": 600 }]));
}

#[test]
fn a_first_run_code_is_made_and_shown_only_on_a_terminal() {
    use super::super::pair_cli::first_run_notice;
    use super::super::pairing::LINK_PREFIX;
    let (_dir, node) = home();
    let live = || node.remote.pairing().live(Instant::now());

    // Stderr is a log: a service manager's or a hosting platform's.
    let logged = first_run_notice(&node.remote, false).unwrap();
    assert!(live().is_empty(), "no code is made for a log to hold");
    assert!(!logged.contains(LINK_PREFIX), "{logged}");
    assert!(logged.contains("`ob-node pair`"), "the log says how to get a code: {logged}");

    // Stderr is a terminal: the person in front of it gets the code.
    let shown = first_run_notice(&node.remote, true).unwrap();
    let codes = live();
    assert_eq!(codes.len(), 1);
    let link = node.remote.link_for(&codes[0]);
    assert!(shown.contains(&words(&codes[0].secret).join(" ")), "the words");
    assert!(shown.contains(&link) && link.starts_with(LINK_PREFIX), "the link");
    let onion = pairing_onion(&codes[0].secret);
    for secret in [
        words(&codes[0].secret).join(" "),
        base32(&codes[0].secret),
        encode_hex(&codes[0].secret),
        link,
        onion.trim_end_matches(".onion").to_string(),
    ] {
        assert!(!logged.contains(&secret), "the log line holds part of a code");
    }
}

#[test]
fn the_pairing_log_tells_what_happened_and_never_holds_the_code() {
    use super::super::pair_log::tests::LINES;
    let (_dir, node) = home();
    LINES.take();
    let code = node.remote.issue_code(None).unwrap();
    let link = node.remote.link_for(&code);
    let (status_line, asked) = Ask::phone("Field phone").send(&node, &code.secret);
    assert!(status_line.contains("202"), "{status_line}");
    pair_http::decide(&node, asked["requestId"].as_str().unwrap(), true).unwrap();
    let rejected = node.remote.issue_code(None).unwrap();
    let (_, asked) = Ask::phone("Stranger").send(&node, &rejected.secret);
    pair_http::decide(&node, asked["requestId"].as_str().unwrap(), false).unwrap();
    let unused = node.remote.issue_code(None).unwrap();
    node.remote.pairing().live(Instant::now() + CODE_LIFETIME);

    let lines = LINES.take();
    let onion = pairing_onion(&code.secret);
    let short = format!("{}…{}.onion", &onion[..6], &onion[52..56]);
    let device = &node.remote.devices().list()[0].id.clone();
    assert_eq!(
        lines,
        [
            format!("code {} (onion {short}) made by the node itself; it has no onion (Tor is off, or the onion could not start)", code.id),
            format!("code {}: a request from \"Field phone\" (ios) arrived by a pairing onion; it waits for the owner", code.id),
            format!("code {}: the request from \"Field phone\" was approved as device {device}", code.id),
            lines[3].clone(),
            format!("code {}: a request from \"Stranger\" (ios) arrived by a pairing onion; it waits for the owner", rejected.id),
            format!("code {}: the request from \"Stranger\" was rejected", rejected.id),
            lines[6].clone(),
            format!("code {} expired unused", unused.id),
        ]
    );
    let story = lines.join("\n");
    for code in [&code, &rejected, &unused] {
        let onion = pairing_onion(&code.secret);
        for secret in [
            words(&code.secret).join(" "),
            base32(&code.secret),
            encode_hex(&code.secret),
            node.remote.link_for(code),
            onion.trim_end_matches(".onion").to_string(),
        ] {
            assert!(!story.contains(&secret), "a pairing line holds part of a code");
        }
    }
    assert!(link.contains(&base32(&code.secret)), "the link is the secret these lines leave out");
}
