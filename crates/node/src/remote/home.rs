//! The node as a remote client of one home node.
//!
//! The desktop app's own node pairs with a home node and keeps the device
//! secret sealed in `home-node.obv`. The shop window then sends
//! `/home/proxy/{path}` to its own node with the pairing token, as for any
//! other route; this node signs the request with the device key and sends
//! it to the home node's onion over Tor, and hands back the answer. The
//! window never holds the device key and never dials anything itself.
//!
//! | route | body | answer |
//! |---|---|---|
//! | `GET /home` | | `{paired, address, deviceId, name, pairedAt}` |
//! | `POST /home/pair` | `{code, name}` | the same, once paired |
//! | `DELETE /home` | | forgets the home node, after asking it to remove this device |
//! | any `/home/proxy/{path}` | any | the home node's answer to `{path}` |
//!
//! Proxy requests run on their own thread, so a slow Tor circuit does not
//! hold up the loopback server.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tormail::OnionAddress;
use zeroize::Zeroizing;

use super::auth::{decode_hex, pair_digest, DeviceKey};
use super::devices::clean_name;
use super::now_secs;
use super::offers::parse_code;
use crate::http::{http_response_cors, json_body, json_error, local_guard, Client};
use crate::http_parse::header;
use crate::vault::Vault;
use crate::vault_key::{encode_hex, random_bytes};
use crate::{Error, Node};

const FILE: &str = "home-node.obv";
pub(crate) const PROXY_PREFIX: &str = "/home/proxy";
const MAX_ANSWER: usize = 8 * 1024 * 1024;
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(20);

struct Profile {
    address: String,
    node_id: String,
    secret: Zeroizing<[u8; 32]>,
    device_id: String,
    name: String,
    paired_at: u64,
}

impl Profile {
    fn key(&self) -> DeviceKey {
        DeviceKey::from_secret(&self.secret).expect("a stored device secret is a valid key")
    }

    fn value(&self) -> Value {
        json!({
            "paired": true,
            "address": self.address,
            "deviceId": self.device_id,
            "name": self.name,
            "pairedAt": self.paired_at,
        })
    }
}

pub(crate) struct HomeState {
    path: PathBuf,
    vault: Vault,
    profile: Mutex<Option<Profile>>,
    /// Home node clock minus ours, learned from a `clock` refusal.
    offset: AtomicI64,
}

impl HomeState {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let profile = match vault.read_sealed(&path)? {
            None => None,
            Some(bytes) => {
                let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail("home-node.obv could not be read".into()))?;
                let secret: [u8; 32] = decode_hex(value["secret"].as_str().unwrap_or_default())
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| Error::Detail("home-node.obv has no device secret".into()))?;
                let key = DeviceKey::from_secret(&secret).ok_or_else(|| Error::Detail("home-node.obv has a bad device secret".into()))?;
                Some(Profile {
                    address: value["address"].as_str().unwrap_or_default().to_string(),
                    node_id: value["nodeId"].as_str().unwrap_or_default().to_string(),
                    device_id: key.id(),
                    secret: Zeroizing::new(secret),
                    name: value["name"].as_str().unwrap_or_default().to_string(),
                    paired_at: value["pairedAt"].as_u64().unwrap_or(0),
                })
            }
        };
        Ok(Self { path, vault, profile: Mutex::new(profile), offset: AtomicI64::new(0) })
    }

    fn profile(&self) -> MutexGuard<'_, Option<Profile>> {
        self.profile.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn save(&self, profile: Profile) -> Result<Value, Error> {
        let body = Zeroizing::new(
            json!({
                "v": 1,
                "address": profile.address,
                "nodeId": profile.node_id,
                "secret": encode_hex(profile.secret.as_ref()),
                "name": profile.name,
                "pairedAt": profile.paired_at,
            })
            .to_string(),
        );
        self.vault.write_sealed(&self.path, body.as_bytes())?;
        let value = profile.value();
        *self.profile() = Some(profile);
        Ok(value)
    }

    fn forget(&self) -> Result<(), Error> {
        *self.profile() = None;
        match std::fs::remove_file(&self.path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(Error::Detail(err.to_string())),
            _ => Ok(()),
        }
    }
}

/// True for a raw request whose target is under `/home/proxy/`.
pub(crate) fn is_proxy_request(raw: &[u8]) -> bool {
    let line = raw.split(|b| *b == b'\n').next().unwrap_or_default();
    let mut parts = line.split(|b| *b == b' ');
    parts.next();
    parts.next().is_some_and(|target| target.starts_with(b"/home/proxy/"))
}

pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, path: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let home = &node.remote.home;
    match (method, path) {
        ("GET", "/home") => {
            let value = home.profile().as_ref().map(Profile::value).unwrap_or_else(|| json!({ "paired": false }));
            json_body("200 OK", &value, origin)
        }
        ("POST", "/home/pair") => pair(node, body, origin),
        ("DELETE", "/home") => {
            let device = home.profile().as_ref().map(|profile| profile.device_id.clone());
            if let Some(device) = device {
                if let Err(message) = send(node, "DELETE", &format!("/remote/devices/{device}"), None, b"") {
                    eprintln!("ob-node: the home node was not told to remove this device: {message}");
                }
            }
            match home.forget() {
                Ok(()) => json_body("200 OK", &json!({ "paired": false }), origin),
                Err(err) => json_error("500 Internal Server Error", &err.to_string(), origin),
            }
        }
        (_, path) if path.starts_with(PROXY_PREFIX) => {
            let target = &path[PROXY_PREFIX.len()..];
            if !target.starts_with('/') || !target.bytes().all(|b| b.is_ascii_graphic()) {
                return json_error("400 Bad Request", "That path cannot be sent to the home node.", origin);
            }
            match send(node, method, target, header(head, "content-type"), body) {
                Ok((status, content_type, answer)) => http_response_cors(&status, &content_type, &answer, origin),
                Err(message) => json_body("502 Bad Gateway", &json!({ "error": message, "code": "home_unreachable" }), origin),
            }
        }
        _ => json_error("404 Not Found", "not found", origin),
    }
}

fn pair(node: &Node, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The request could not be read.", origin);
    };
    let Some((secret, address)) = value["code"].as_str().and_then(parse_code) else {
        return json_error("400 Bad Request", "That is not a home node pairing code. It starts with obhome:.", origin);
    };
    let Some(name) = value["name"].as_str().and_then(clean_name) else {
        return json_error("400 Bad Request", "Name this device in 1 to 40 characters.", origin);
    };
    if let Err(message) = dial_target(node, &address) {
        return json_error("400 Bad Request", &message, origin);
    }
    let device_secret = Zeroizing::new(random_bytes::<32>());
    let Some(key) = DeviceKey::from_secret(&device_secret) else {
        return json_error("500 Internal Server Error", "A device key could not be made. Try again.", origin);
    };
    let key_hex = encode_hex(&key.public().serialize());
    let request = json!({
        "secret": secret,
        "publicKey": key_hex,
        "name": name,
        "signature": key.sign(pair_digest(&secret, &key_hex, &name)),
    })
    .to_string();
    let raw = format!(
        "POST /remote/pair HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{request}",
        request.len()
    );
    let answer = match exchange(node, &address, raw.as_bytes()).and_then(|bytes| parse_answer(&bytes)) {
        Ok(answer) => answer,
        Err(message) => return json_body("502 Bad Gateway", &json!({ "error": message, "code": "home_unreachable" }), origin),
    };
    let (status, _, body) = answer;
    let reply: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if !status.starts_with("200") {
        let message = reply["error"].as_str().unwrap_or("The home node refused the pairing code.");
        return json_error("400 Bad Request", message, origin);
    }
    let node_id = reply["nodeId"].as_str().unwrap_or_default().to_string();
    if reply["deviceId"].as_str() != Some(key.id().as_str()) || node_id.len() != 32 {
        return json_error("502 Bad Gateway", "The home node's answer did not match this device.", origin);
    }
    node.remote.home.offset.store(0, Ordering::Relaxed);
    let profile = Profile { address, node_id, device_id: key.id(), secret: device_secret, name, paired_at: now_secs() };
    eprintln!("ob-node: paired with home node {} as device {}", profile.address, profile.device_id);
    match node.remote.home.save(profile) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => json_error("500 Internal Server Error", &err.to_string(), origin),
    }
}

/// Sign and send one request to the paired home node. Retries once with
/// the home node's clock when it says ours is off.
fn send(node: &Node, method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> Result<(String, String, Vec<u8>), String> {
    let home = &node.remote.home;
    let (address, node_id, key) = {
        let profile = home.profile();
        let profile = profile.as_ref().ok_or("This node is not paired with a home node.")?;
        (profile.address.clone(), profile.node_id.clone(), profile.key())
    };
    for attempt in 0..2 {
        let time = now_secs().saturating_add_signed(home.offset.load(Ordering::Relaxed));
        let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
        raw.push_str(&key.headers(&node_id, time, method, path, body));
        if let Some(content_type) = content_type {
            raw.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        raw.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(body);
        let answer = parse_answer(&exchange(node, &address, &raw)?)?;
        if attempt == 0 && answer.0.starts_with("401") {
            let reply: Value = serde_json::from_slice(&answer.2).unwrap_or(Value::Null);
            if let (Some("clock"), Some(server)) = (reply["code"].as_str(), reply["serverTime"].as_u64()) {
                home.offset.store(server as i64 - now_secs() as i64, Ordering::Relaxed);
                continue;
            }
        }
        return Ok(answer);
    }
    Err("The home node's clock could not be matched.".into())
}

enum Target {
    Onion(OnionAddress),
    Loopback(SocketAddr),
}

fn dial_target(node: &Node, address: &str) -> Result<Target, String> {
    if address.ends_with(".onion") {
        return OnionAddress::parse(address).map(Target::Onion).map_err(|_| "That onion address is not valid.".into());
    }
    match address.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_loopback() && node.remote.config().loopback_homes => Ok(Target::Loopback(addr)),
        _ => Err("A home node is reached only at its onion address.".into()),
    }
}

fn exchange(node: &Node, address: &str, request: &[u8]) -> Result<Vec<u8>, String> {
    match dial_target(node, address)? {
        Target::Onion(onion) => node.remote.tor(&node.data_dir)?.exchange(&onion, request, MAX_ANSWER),
        Target::Loopback(addr) => {
            let io = |err: std::io::Error| format!("the home node could not be reached: {err}");
            let mut stream = TcpStream::connect_timeout(&addr, LOOPBACK_TIMEOUT).map_err(io)?;
            stream.set_read_timeout(Some(LOOPBACK_TIMEOUT)).map_err(io)?;
            stream.write_all(request).map_err(io)?;
            let mut answer = Vec::new();
            stream.take(MAX_ANSWER as u64 + 1).read_to_end(&mut answer).map_err(io)?;
            if answer.len() > MAX_ANSWER {
                return Err("The home node's answer is too large.".into());
            }
            Ok(answer)
        }
    }
}

/// `(status, content type, body)` of one HTTP/1.1 answer.
fn parse_answer(bytes: &[u8]) -> Result<(String, String, Vec<u8>), String> {
    let bad = || "The home node sent an answer this node could not read.".to_string();
    let split = bytes.windows(4).position(|window| window == b"\r\n\r\n").ok_or_else(bad)?;
    let head = std::str::from_utf8(&bytes[..split]).map_err(|_| bad())?;
    let body = &bytes[split + 4..];
    let status = head.lines().next().and_then(|line| line.strip_prefix("HTTP/1.1 ")).ok_or_else(bad)?.to_string();
    if let Some(length) = header(head, "content-length") {
        if length.parse::<usize>().ok() != Some(body.len()) {
            return Err(bad());
        }
    }
    let content_type = header(head, "content-type").unwrap_or("application/octet-stream").to_string();
    Ok((status, content_type, body.to_vec()))
}
