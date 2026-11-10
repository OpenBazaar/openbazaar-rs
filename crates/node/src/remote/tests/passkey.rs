//! Passkey sign-in on the web gateway, with an authenticator made of a P-256 key.

use bitcoin::hashes::{sha256, Hash};
use p256::ecdsa::signature::Signer as _;

use super::web::{browser, on_web, web_home, HOST, HTTPS};
use super::*;
use crate::remote::passkeys::bound_challenge;

const CREDENTIAL: &str = "c2Vzc2lvbi1vbmU";
const VERIFIED: u8 = 0x05;

struct Authenticator(p256::ecdsa::SigningKey);

impl Authenticator {
    fn new() -> Self {
        Self(p256::ecdsa::SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng))
    }

    fn key(&self) -> String {
        DeviceKey::P256(*self.0.verifying_key()).text()
    }

    /// What `navigator.credentials.get` returns for `challenge` on a page of `origin`.
    fn assert(&self, challenge: &[u8; 32], origin: &str, flags: u8) -> Value {
        let client = json!({ "type": "webauthn.get", "challenge": data_encoding::BASE64URL_NOPAD.encode(challenge), "origin": origin, "crossOrigin": false }).to_string();
        let mut data = sha256::Hash::hash(HOST.as_bytes()).to_byte_array().to_vec();
        data.push(flags);
        data.extend_from_slice(&[0, 0, 0, 0]);
        let mut signed = data.clone();
        signed.extend_from_slice(&sha256::Hash::hash(client.as_bytes()).to_byte_array());
        let signature: p256::ecdsa::Signature = self.0.sign(&signed);
        json!({
            "authenticatorData": encode_hex(&data),
            "clientDataJSON": encode_hex(client.as_bytes()),
            "signature": encode_hex(signature.to_der().as_bytes()),
        })
    }
}

fn post(node: &Node, path: &str, body: &Value) -> (String, Value) {
    on_web(node, &unsigned("POST", path, "Content-Type: application/json\r\n", &body.to_string()), HTTPS)
}

fn challenge(node: &Node) -> [u8; 32] {
    let (status, body) = post(node, "/web/passkey/challenge", &json!({}));
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(body["rpId"], HOST);
    crate::remote::auth::decode_hex(body["challenge"].as_str().unwrap()).unwrap().try_into().unwrap()
}

/// A browser's whole sign-in: a fresh session key, approved by `authenticator`.
fn sign_in(node: &Node, authenticator: &Authenticator) -> ((String, Value), Device) {
    let session = Device { key: Key::bip340(), watch: None, node_id: node.remote.node_id() };
    let challenge = challenge(node);
    let key = session.key.public().text();
    let mut body = authenticator.assert(&bound_challenge(&challenge, &key), &format!("https://{HOST}"), VERIFIED);
    body["challenge"] = json!(encode_hex(&challenge));
    body["key"] = json!(key);
    body["credentialId"] = json!(CREDENTIAL);
    (post(node, "/web/passkey/signin", &body), session)
}

fn register(node: &Node, owner: &Device, authenticator: &Authenticator) {
    let body = json!({ "id": CREDENTIAL, "key": authenticator.key() }).to_string();
    let (status, list) = on_web(node, &owner.raw("POST", "/remote/passkeys", &body, now_secs()), HTTPS);
    assert!(status.contains("200"), "{status} {list}");
    assert_eq!(list["passkeys"][0]["id"], CREDENTIAL);
}

fn hello(node: &Node) -> Value {
    on_web(node, &unsigned("GET", "/remote/hello", "", ""), HTTPS).1
}

#[test]
fn a_passkey_signs_a_browser_in_and_removing_it_signs_the_browser_out() {
    let (_dir, node) = web_home();
    let owner = browser(&node);
    let authenticator = Authenticator::new();
    assert_eq!(hello(&node)["passkeys"], false);
    register(&node, &owner, &authenticator);
    assert_eq!(hello(&node)["passkeys"], true);

    let ((status, body), session) = sign_in(&node, &authenticator);
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(body["deviceId"], session.id());
    let (status, devices) = on_web(&node, &session.raw("GET", "/remote/devices", "", now_secs()), HTTPS);
    assert!(status.contains("200"), "{status} {devices}");
    let me = devices["devices"].as_array().unwrap().iter().find(|device| device["thisDevice"] == true).unwrap();
    assert_eq!(me["pairedBy"], format!("passkey:{CREDENTIAL}"));

    let (status, list) = on_web(&node, &owner.raw("DELETE", &format!("/remote/passkeys/{CREDENTIAL}"), "", now_secs()), HTTPS);
    assert!(status.contains("200") && list["passkeys"] == json!([]), "{status} {list}");
    let (status, _) = on_web(&node, &session.raw("GET", "/remote/devices", "", now_secs()), HTTPS);
    assert!(status.contains("401"), "the session went with its passkey: {status}");
    let ((status, _), _) = sign_in(&node, &authenticator);
    assert!(status.contains("401"), "{status}");
    assert_eq!(hello(&node)["passkeys"], false);
}

#[test]
fn a_passkey_answer_is_for_one_challenge_one_session_key_and_this_address() {
    let (_dir, node) = web_home();
    let owner = browser(&node);
    let authenticator = Authenticator::new();
    register(&node, &owner, &authenticator);
    let origin = format!("https://{HOST}");
    let session = Key::bip340().public().text();
    let attempt = |challenge: &[u8; 32], signed: &[u8; 32], origin: &str, flags: u8, by: &Authenticator, key: &str| {
        let mut body = by.assert(signed, origin, flags);
        body["challenge"] = json!(encode_hex(challenge));
        body["key"] = json!(key);
        body["credentialId"] = json!(CREDENTIAL);
        post(&node, "/web/passkey/signin", &body)
    };

    let fresh = challenge(&node);
    let (status, body) = attempt(&fresh, &bound_challenge(&fresh, &session), "https://other.example.com", VERIFIED, &authenticator, &session);
    assert!(status.contains("401"), "another page's answer: {status} {body}");
    let fresh = challenge(&node);
    let (status, _) = attempt(&fresh, &bound_challenge(&fresh, &session), &origin, 0x01, &authenticator, &session);
    assert!(status.contains("401"), "the user was not verified: {status}");
    let fresh = challenge(&node);
    let (status, _) = attempt(&fresh, &bound_challenge(&fresh, &session), &origin, VERIFIED, &Authenticator::new(), &session);
    assert!(status.contains("401"), "another authenticator: {status}");
    let fresh = challenge(&node);
    let other = Key::bip340().public().text();
    let (status, _) = attempt(&fresh, &bound_challenge(&fresh, &session), &origin, VERIFIED, &authenticator, &other);
    assert!(status.contains("401"), "the answer names another session key: {status}");
    let made_up = [7u8; 32];
    let (status, _) = attempt(&made_up, &bound_challenge(&made_up, &session), &origin, VERIFIED, &authenticator, &session);
    assert!(status.contains("401"), "a challenge the node never issued: {status}");

    let fresh = challenge(&node);
    let (status, _) = attempt(&fresh, &bound_challenge(&fresh, &session), &origin, VERIFIED, &authenticator, &session);
    assert!(status.contains("200"), "{status}");
    let (status, _) = attempt(&fresh, &bound_challenge(&fresh, &session), &origin, VERIFIED, &authenticator, &session);
    assert!(status.contains("401"), "a challenge is used once: {status}");

    // The same vector as the web app's test: both sides hash the same text.
    let vector = bound_challenge(&[7u8; 32], &format!("bip340:{}", "ab".repeat(32)));
    assert_eq!(encode_hex(&vector), "8bfc3cded9cbeff69929711ce725eb1f03506650d8e69dbf42c4e2ebca1a58f3");

    // The sign-in routes exist only on the web gateway.
    let raw = unsigned("POST", "/web/passkey/challenge", "", "");
    let (status, _) = send(&node, Entry::Admin, &raw);
    assert!(status.contains("401"), "{status}");
}

/// What the first browser at a new node sends: the passkey it made and its session key.
fn claim(node: &Node, authenticator: &Authenticator) -> ((String, Value), Device) {
    let session = Device { key: Key::bip340(), watch: None, node_id: node.remote.node_id() };
    let body = json!({ "credentialId": CREDENTIAL, "passkey": authenticator.key(), "key": session.key.public().text() });
    (post(node, "/web/passkey/claim", &body), session)
}

#[test]
fn the_first_browser_at_a_new_node_owns_it_and_the_second_does_not() {
    let (_dir, node) = web_home();
    let authenticator = Authenticator::new();
    assert_eq!(hello(&node)["claimable"], true);
    let body = json!({ "credentialId": CREDENTIAL, "passkey": authenticator.key(), "key": Key::bip340().public().text() }).to_string();
    let (status, _) = send(&node, Entry::Admin, &unsigned("POST", "/web/passkey/claim", "Content-Type: application/json\r\n", &body));
    assert!(status.contains("401"), "only the web address is claimed: {status}");

    let ((status, body), session) = claim(&node, &authenticator);
    assert!(status.contains("200"), "{status} {body}");
    assert_eq!(body["deviceId"], session.id());
    let (status, list) = on_web(&node, &session.raw("GET", "/remote/passkeys", "", now_secs()), HTTPS);
    assert!(status.contains("200") && list["passkeys"][0]["id"] == CREDENTIAL, "{status} {list}");
    assert_eq!(hello(&node)["claimable"], false);

    let ((status, body), stranger) = claim(&node, &Authenticator::new());
    assert!(status.contains("403"), "{status} {body}");
    let (status, _) = on_web(&node, &stranger.raw("GET", "/remote/devices", "", now_secs()), HTTPS);
    assert!(status.contains("401"), "{status}");
    assert_eq!(node.remote.devices().passkeys.len(), 1);

    // The owner's passkey signs in from then on.
    let ((status, body), _) = sign_in(&node, &authenticator);
    assert!(status.contains("200"), "{status} {body}");
}

#[test]
fn a_node_with_an_access_key_or_a_wallet_is_not_claimed() {
    let (_dir, node) = web_home();
    browser(&node);
    assert_eq!(hello(&node)["claimable"], false);
    let ((status, _), _) = claim(&node, &Authenticator::new());
    assert!(status.contains("403"), "{status}");

    let (_dir, node) = web_home();
    crate::harness::unlock(&node);
    assert_eq!(hello(&node)["claimable"], false);
    let ((status, _), _) = claim(&node, &Authenticator::new());
    assert!(status.contains("403"), "{status}");
    assert!(node.remote.devices().passkeys.is_empty() && node.remote.devices().list().is_empty());
}

#[test]
fn old_passkey_sessions_make_room_and_passkeys_survive_a_restart() {
    let dir = TempDir::new();
    let authenticator = Authenticator::new();
    {
        let node = open_home(&dir);
        node.set_remote(super::web::flags(&["--web-listen", "0.0.0.0:8080", "--web-origin", &format!("https://{HOST}")]).unwrap()).unwrap();
        let owner = browser(&node);
        register(&node, &owner, &authenticator);
        for _ in 0..9 {
            let ((status, body), _) = sign_in(&node, &authenticator);
            assert!(status.contains("200"), "{status} {body}");
        }
        let sessions = node.remote.devices().list().iter().filter(|device| device.paired_by.is_some()).count();
        assert_eq!(sessions, 8, "the ninth sign-in took the place of an idle session");
    }
    let node = open_home(&dir);
    assert_eq!(node.remote.devices().passkeys.len(), 1);
    assert_eq!(node.remote.devices().passkeys[0].id, CREDENTIAL);
}
