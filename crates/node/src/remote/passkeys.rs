//! Passkeys (WebAuthn) for the web gateway ([`super::web_origin`]).
//!
//! A passkey cannot sign every request: its authenticator asks the user for
//! each signature. It approves a browser instead, the way the owner approves
//! a pairing on the node. The browser makes a device key, the passkey signs
//! a challenge that names that key, and the key joins the device list as a
//! session of that passkey. From then on the browser signs its requests like
//! any device. A passkey signs in; it does not unlock the wallet.
//!
//! A passkey is registered by a device that is already in, with a signed
//! request that carries the credential id and its P-256 public key. No
//! attestation is checked: that device's signature is the authority.
//! Removing a passkey removes its sessions.
//!
//! A new node has no such device. While it has no device, no passkey, and no
//! wallet, nobody owns it, and the first browser to open its web address
//! registers the first passkey and becomes its first session ([`claim`]).
//! Anyone who knows the address can be that browser. An owner who finds the
//! node taken deletes its data and starts again; nothing is in it yet.
//!
//! | route | caller | answer |
//! |---|---|---|
//! | `POST /web/passkey/challenge` | anyone, on the web gateway | `{challenge, rpId}` |
//! | `POST /web/passkey/signin` | a browser with a passkey | `{deviceId}` |
//! | `POST /web/passkey/claim` | the first browser at a node nobody owns | `{deviceId}` |
//! | `GET /remote/passkeys` | a device | `{enabled, rpId, passkeys}` |
//! | `POST /remote/passkeys` | a device | the list with the new passkey |
//! | `DELETE /remote/passkeys/{id}` | a device | the list without it |

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bitcoin::hashes::{sha256, Hash};
use serde_json::{json, Value};

use super::auth::decode_hex;
use super::devices::NewDevice;
use super::protocol::DeviceKey;
use super::web_origin::WebOrigin;
use super::now_secs;
use crate::http::{json_body, json_error};
use crate::vault_key::{encode_hex, random_bytes};
use crate::wallet_lock::State;
use crate::Node;

const MAX_PASSKEYS: usize = 8;
/// Browsers one node keeps signed in by passkey. The one unused longest goes first.
const MAX_SESSIONS: usize = 8;
const MAX_CHALLENGES: usize = 64;
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_CREDENTIAL_ID: usize = 512;
const PASSKEY_NAME: &str = "Passkey";
const SESSION_NAME: &str = "Passkey sign-in";
/// `pairedBy` of a session: `passkey:` and the credential id.
const SESSION_PREFIX: &str = "passkey:";
/// Flags in authenticator data: the user was present, and was verified.
const USER_PRESENT_AND_VERIFIED: u8 = 0x05;

#[derive(Clone, Debug)]
pub(crate) struct Passkey {
    /// The WebAuthn credential id, base64url as the browser names it.
    pub id: String,
    pub key: DeviceKey,
    pub added_at: u64,
}

impl Passkey {
    pub(crate) fn stored(&self) -> Value {
        json!({ "id": self.id, "key": self.key.text(), "addedAt": self.added_at })
    }

    pub(crate) fn load(item: &Value) -> Option<Self> {
        Some(Self {
            id: item["id"].as_str().filter(|id| credential_id(id))?.to_string(),
            key: p256_key(item["key"].as_str()?)?,
            added_at: item["addedAt"].as_u64().unwrap_or(0),
        })
    }

    fn value(&self) -> Value {
        json!({ "id": self.id, "name": PASSKEY_NAME, "addedAt": self.added_at })
    }
}

fn credential_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CREDENTIAL_ID
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn p256_key(text: &str) -> Option<DeviceKey> {
    DeviceKey::parse(text).filter(|key| matches!(key, DeviceKey::P256(_)))
}

/// Sign-in challenges this node issued and has not seen answered. Memory only.
#[derive(Default)]
pub(crate) struct Challenges(Mutex<VecDeque<([u8; 32], Instant)>>);

impl Challenges {
    fn issue(&self, now: Instant) -> [u8; 32] {
        let mut live = self.0.lock().unwrap_or_else(|err| err.into_inner());
        live.retain(|(_, made)| now.duration_since(*made) < CHALLENGE_LIFETIME);
        if live.len() >= MAX_CHALLENGES {
            live.pop_front();
        }
        let challenge = random_bytes::<32>();
        live.push_back((challenge, now));
        challenge
    }

    /// True once for a challenge that is still live.
    fn take(&self, challenge: &[u8; 32], now: Instant) -> bool {
        let mut live = self.0.lock().unwrap_or_else(|err| err.into_inner());
        live.retain(|(_, made)| now.duration_since(*made) < CHALLENGE_LIFETIME);
        let found = live.iter().position(|(known, _)| known == challenge);
        found.is_some_and(|index| live.remove(index).is_some())
    }
}

/// What the authenticator signs: the node's challenge and the key of the
/// browser session it approves.
pub(crate) fn bound_challenge(challenge: &[u8; 32], session_key: &str) -> [u8; 32] {
    let text = format!("ob/passkey/v1\n{}\n{session_key}", encode_hex(challenge));
    sha256::Hash::hash(text.as_bytes()).to_byte_array()
}

/// A WebAuthn assertion as the browser returns it.
struct Assertion {
    authenticator_data: Vec<u8>,
    client_data: Vec<u8>,
    signature: Vec<u8>,
}

/// Check that `key` signed `expected` for this origin, with the user verified.
fn verify(key: &DeviceKey, origin: &WebOrigin, expected: &[u8; 32], assertion: &Assertion) -> Result<(), &'static str> {
    let client: Value = serde_json::from_slice(&assertion.client_data).map_err(|_| "The passkey answer could not be read.")?;
    let challenge = data_encoding::BASE64URL_NOPAD.encode(expected);
    if client["type"] != "webauthn.get"
        || client["challenge"].as_str() != Some(&challenge)
        || client["origin"].as_str() != Some(origin.as_str())
        || client["crossOrigin"] == true
    {
        return Err("The passkey answered a different request.");
    }
    let data = &assertion.authenticator_data;
    let relying_party = sha256::Hash::hash(origin.rp_id().as_bytes()).to_byte_array();
    if data.len() < 37 || data[..32] != relying_party {
        return Err("The passkey is for a different address.");
    }
    if data[32] & USER_PRESENT_AND_VERIFIED != USER_PRESENT_AND_VERIFIED {
        return Err("The passkey did not verify you. Use one with a fingerprint, face, or PIN.");
    }
    let mut signed = data.clone();
    signed.extend_from_slice(&sha256::Hash::hash(&assertion.client_data).to_byte_array());
    if !key.verifies(&signed, &encode_hex(&assertion.signature)) {
        return Err("The passkey signature did not verify.");
    }
    Ok(())
}

fn refused(message: &str) -> Vec<u8> {
    json_body("401 Unauthorized", &json!({ "error": message, "code": "passkey" }), None)
}

/// The sign-in routes, which need no signature. Only the web gateway serves them.
pub(crate) fn public_response(node: &Node, origin: &WebOrigin, method: &str, path: &str, body: &[u8]) -> Option<Vec<u8>> {
    match (method, path) {
        ("POST", "/web/passkey/challenge") => {
            let challenge = node.remote.passkey_challenges.issue(Instant::now());
            Some(json_body("200 OK", &json!({ "challenge": encode_hex(&challenge), "rpId": origin.rp_id() }), None))
        }
        ("POST", "/web/passkey/signin") => Some(sign_in(node, origin, body)),
        ("POST", "/web/passkey/claim") => Some(claim(node, body)),
        _ => None,
    }
}

/// The browser session a passkey approved.
fn session_of(key: DeviceKey, passkey_id: &str) -> NewDevice {
    NewDevice {
        key,
        watch: None,
        discovery: None,
        name: SESSION_NAME.into(),
        platform: "browser".into(),
        paired_by: Some(format!("{SESSION_PREFIX}{passkey_id}")),
    }
}

/// Nobody owns this node yet: it has no device, no passkey, and no wallet.
pub(crate) fn claimable(node: &Node) -> bool {
    let no_wallet = node.lock.state() == State::Setup;
    let devices = node.remote.devices();
    no_wallet && devices.list().is_empty() && devices.passkeys.is_empty()
}

/// `POST /web/passkey/claim` with `{credentialId, passkey, key}`: the first
/// passkey of a node nobody owns, and the session key of the browser that made it.
fn claim(node: &Node, body: &[u8]) -> Vec<u8> {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let fields = (|| {
        let id = value["credentialId"].as_str().filter(|id| credential_id(id))?;
        let key = p256_key(value["passkey"].as_str()?)?;
        Some((id, key, DeviceKey::parse(value["key"].as_str()?)?))
    })();
    let Some((id, key, session)) = fields else {
        return json_error("400 Bad Request", "The passkey could not be read.", None);
    };
    let no_wallet = node.lock.state() == State::Setup;
    // One lock from the check to the save: of two first visitors, one wins.
    let mut devices = node.remote.devices();
    if !no_wallet || !devices.list().is_empty() || !devices.passkeys.is_empty() {
        return json_error("403 Forbidden", "This node already has an owner. Sign in instead.", None);
    }
    devices.passkeys.push(Passkey { id: id.to_string(), key, added_at: now_secs() });
    match devices.add(session_of(session, id), now_secs()) {
        Ok(device) => {
            eprintln!("ob-node: a browser on the web address created this node's first passkey and now owns it");
            json_body("200 OK", &json!({ "deviceId": device.id }), None)
        }
        Err(message) => {
            devices.passkeys.clear();
            json_error("500 Internal Server Error", &message, None)
        }
    }
}

fn sign_in(node: &Node, origin: &WebOrigin, body: &[u8]) -> Vec<u8> {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let bytes = |name: &str| value[name].as_str().and_then(decode_hex);
    let fields = (|| {
        let challenge: [u8; 32] = bytes("challenge")?.try_into().ok()?;
        let assertion = Assertion {
            authenticator_data: bytes("authenticatorData")?,
            client_data: bytes("clientDataJSON")?,
            signature: bytes("signature")?,
        };
        let session = DeviceKey::parse(value["key"].as_str()?)?;
        Some((challenge, assertion, session, value["credentialId"].as_str()?))
    })();
    let Some((challenge, assertion, session, credential)) = fields else {
        return json_error("400 Bad Request", "The passkey sign-in could not be read.", None);
    };
    if !node.remote.passkey_challenges.take(&challenge, Instant::now()) {
        return refused("That sign-in took too long. Try again.");
    }
    let Some(passkey) = node.remote.devices().passkeys.iter().find(|passkey| passkey.id == credential).cloned() else {
        return refused("This node does not know that passkey.");
    };
    if let Err(message) = verify(&passkey.key, origin, &bound_challenge(&challenge, &session.text()), &assertion) {
        return refused(message);
    }
    make_room(node);
    let added = node.remote.devices().add(session_of(session, &passkey.id), now_secs());
    match added {
        Ok(device) => json_body("200 OK", &json!({ "deviceId": device.id }), None),
        Err(message) => json_error("409 Conflict", &message, None),
    }
}

/// Before a new session: drop the sessions unused longest, down to one
/// under [`MAX_SESSIONS`]. A session that paired a device stays, because
/// removing it would remove that device.
fn make_room(node: &Node) {
    let mut idle: Vec<(u64, String)> = {
        let devices = node.remote.devices();
        let is_session = |by: &Option<String>| by.as_deref().is_some_and(|by| by.starts_with(SESSION_PREFIX));
        let sessions = devices.list().iter().filter(|device| is_session(&device.paired_by));
        let childless = sessions.filter(|session| !devices.list().iter().any(|device| device.paired_by.as_deref() == Some(&session.id)));
        childless.map(|session| (session.last_seen, session.id.clone())).collect()
    };
    idle.sort();
    let extra = (idle.len() + 1).saturating_sub(MAX_SESSIONS);
    for (_, id) in idle.into_iter().take(extra) {
        let _ = node.remote.revoke(&id);
    }
}

/// `GET /remote/passkeys`.
pub(crate) fn list(node: &Node) -> Value {
    let origin = node.remote.web_origin();
    json!({
        "enabled": origin.is_some(),
        "rpId": origin.map(WebOrigin::rp_id),
        "passkeys": node.remote.devices().passkeys.iter().map(Passkey::value).collect::<Vec<_>>(),
    })
}

/// `POST /remote/passkeys` with `{id, key}`: the credential a signed-in browser just made.
pub(crate) fn add(node: &Node, value: &Value) -> Result<Value, String> {
    if node.remote.web_origin().is_none() {
        return Err("This node has no web address, so a passkey has nothing to sign in to.".into());
    }
    let id = value["id"].as_str().filter(|id| credential_id(id)).ok_or("The passkey id could not be read.")?;
    let key = value["key"].as_str().and_then(p256_key).ok_or("This node takes passkeys with a P-256 key.")?;
    {
        let mut devices = node.remote.devices();
        if devices.passkeys.iter().any(|passkey| passkey.id == id) {
            return Err("That passkey is already on this node.".into());
        }
        if devices.passkeys.len() >= MAX_PASSKEYS {
            return Err(format!("This node already has {MAX_PASSKEYS} passkeys. Remove one first."));
        }
        devices.passkeys.push(Passkey { id: id.to_string(), key, added_at: now_secs() });
        devices.save().map_err(|err| err.to_string())?;
    }
    Ok(list(node))
}

/// `DELETE /remote/passkeys/{id}`: forget the passkey and sign out its
/// sessions. `None` when the node has no such passkey.
pub(crate) fn remove(node: &Node, id: &str) -> Result<Option<Value>, String> {
    let sessions: Vec<String> = {
        let mut devices = node.remote.devices();
        let before = devices.passkeys.len();
        devices.passkeys.retain(|passkey| passkey.id != id);
        if devices.passkeys.len() == before {
            return Ok(None);
        }
        devices.save().map_err(|err| err.to_string())?;
        let by = format!("{SESSION_PREFIX}{id}");
        devices.list().iter().filter(|device| device.paired_by.as_deref() == Some(&by)).map(|device| device.id.clone()).collect()
    };
    for session in sessions {
        node.remote.revoke(&session).map_err(|err| err.to_string())?;
    }
    Ok(Some(list(node)))
}
