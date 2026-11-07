//! The node as a remote client of one home node.
//!
//! The desktop app's own node pairs with a home node and keeps the device
//! secret and the discovery secret sealed in `home-node.obv`. The shop window
//! then sends `/home/proxy/{path}` to its own node with the pairing token, as
//! for any other route; this node signs the request with the device key and
//! sends it to the home node (LAN channel or Tor), and hands back the answer.
//! The window never holds a key and never dials anything itself.
//!
//! | route | body | answer |
//! |---|---|---|
//! | `GET /home` | | `{paired, onion, nodeId, deviceId, name, pairedAt, via}` |
//! | `POST /home/pair` | `{code, name}` | `{requestId, sas, sasNames}`; the owner approves on the home node |
//! | `GET /home/pair/{requestId}` | | waits up to 25 s: `{state}`, and the `GET /home` answer once approved |
//! | `DELETE /home` | | forgets the home node, after asking it to remove this device |
//! | any `/home/proxy/{path}` | any | the home node's answer to `{path}` |
//!
//! Proxy requests run on their own thread, so a slow Tor circuit does not
//! hold up the loopback server.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::auth::{decode_hex, Signer};
use super::home_link::{self, Answer, Route};
use super::home_pair::{self, PendingPair};
use super::now_secs;
use crate::http::{http_response_cors, json_body, json_error, local_guard, Client};
use crate::http_parse::header;
use crate::vault::Vault;
use crate::vault_key::encode_hex;
use crate::{Error, Node};

const FILE: &str = "home-node.obv";
pub(crate) const PROXY_PREFIX: &str = "/home/proxy";

pub(crate) struct Profile {
    pub onion: Option<String>,
    pub node_id: String,
    pub node_key: [u8; 32],
    pub secret: Zeroizing<[u8; 32]>,
    pub discovery: Zeroizing<[u8; 32]>,
    pub lan: Vec<String>,
    pub device_id: String,
    pub name: String,
    pub paired_at: u64,
}

impl Profile {
    fn signer(&self) -> Signer {
        Signer::from_secret(&self.secret).expect("a stored device secret is a valid key")
    }

    fn value(&self, via: Option<&str>) -> Value {
        json!({
            "paired": true,
            "onion": self.onion,
            "nodeId": self.node_id,
            "deviceId": self.device_id,
            "name": self.name,
            "pairedAt": self.paired_at,
            "lan": self.lan,
            "via": via,
        })
    }

    fn stored(&self) -> Value {
        json!({
            "v": 2,
            "onion": self.onion,
            "nodeId": self.node_id,
            "nodeKey": encode_hex(&self.node_key),
            "secret": encode_hex(self.secret.as_ref()),
            "discovery": encode_hex(self.discovery.as_ref()),
            "lan": self.lan,
            "name": self.name,
            "pairedAt": self.paired_at,
        })
    }

    fn load(value: &Value) -> Option<Self> {
        let bytes = |name: &str| {
            decode_hex(value[name].as_str()?).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        };
        let secret = bytes("secret")?;
        Some(Self {
            onion: value["onion"].as_str().map(String::from),
            node_id: value["nodeId"].as_str()?.to_string(),
            node_key: bytes("nodeKey")?,
            device_id: Signer::from_secret(&secret)?.id(),
            secret: Zeroizing::new(secret),
            discovery: Zeroizing::new(bytes("discovery")?),
            lan: value["lan"]
                .as_array()
                .map(|lan| {
                    lan.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            name: value["name"].as_str().unwrap_or_default().to_string(),
            paired_at: value["pairedAt"].as_u64().unwrap_or(0),
        })
    }
}

pub(crate) struct HomeState {
    path: PathBuf,
    vault: Vault,
    profile: Mutex<Option<Profile>>,
    pub(crate) pending: Mutex<Option<PendingPair>>,
    /// Home node clock minus ours, learned from a `clock` refusal.
    offset: AtomicI64,
    /// Whether this process's Tor client has the discovery key.
    discoverable: AtomicBool,
    via: Mutex<Option<&'static str>>,
}

impl HomeState {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let profile = match vault.read_sealed(&path)? {
            None => None,
            Some(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| Error::Detail("home-node.obv could not be read".into()))?;
                match Profile::load(&value) {
                    Some(profile) => Some(profile),
                    None => {
                        eprintln!("ob-node: home-node.obv is from an older version; pair with the home node again");
                        None
                    }
                }
            }
        };
        Ok(Self {
            path,
            vault,
            profile: Mutex::new(profile),
            pending: Mutex::new(None),
            offset: AtomicI64::new(0),
            discoverable: AtomicBool::new(false),
            via: Mutex::new(None),
        })
    }

    fn profile(&self) -> MutexGuard<'_, Option<Profile>> {
        self.profile.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn pending(&self) -> MutexGuard<'_, Option<PendingPair>> {
        self.pending.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn via(&self) -> Option<&'static str> {
        *self.via.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn save(&self, profile: Profile) -> Result<Value, Error> {
        let body = Zeroizing::new(profile.stored().to_string());
        self.vault.write_sealed(&self.path, body.as_bytes())?;
        let value = profile.value(None);
        *self.profile() = Some(profile);
        self.offset.store(0, Ordering::Relaxed);
        self.discoverable.store(false, Ordering::Relaxed);
        Ok(value)
    }

    fn forget(&self) -> Result<(), Error> {
        *self.profile() = None;
        match std::fs::remove_file(&self.path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(Error::Detail(err.to_string()))
            }
            _ => Ok(()),
        }
    }

    /// Follow the home node to its new address.
    fn moved(&self, onion: String) {
        let stored = {
            let mut profile = self.profile();
            let Some(profile) = profile
                .as_mut()
                .filter(|profile| profile.onion.as_deref() != Some(onion.as_str()))
            else {
                return;
            };
            eprintln!("ob-node: the home node moved to {onion}");
            profile.onion = Some(onion);
            profile.stored().to_string()
        };
        self.discoverable.store(false, Ordering::Relaxed);
        if let Err(err) = self
            .vault
            .write_sealed(&self.path, Zeroizing::new(stored).as_bytes())
        {
            eprintln!("ob-node: home-node.obv: {err}");
        }
    }
}

/// True for a raw request whose target is under `/home/proxy/`, or a pairing
/// status wait: both can take as long as Tor does.
pub(crate) fn is_proxy_request(raw: &[u8]) -> bool {
    let line = raw.split(|b| *b == b'\n').next().unwrap_or_default();
    let mut parts = line.split(|b| *b == b' ');
    parts.next();
    parts.next().is_some_and(|target| {
        target.starts_with(b"/home/proxy/") || target.starts_with(b"/home/pair")
    })
}

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    method: &str,
    path: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let home = &node.remote.home;
    match (method, path) {
        ("GET", "/home") => {
            let value = home
                .profile()
                .as_ref()
                .map(|profile| profile.value(home.via()))
                .unwrap_or_else(|| json!({ "paired": false }));
            json_body("200 OK", &value, origin)
        }
        ("POST", "/home/pair") => home_pair::start(node, body, origin),
        ("GET", path) if path.starts_with("/home/pair/") => {
            home_pair::wait(node, &path["/home/pair/".len()..], origin)
        }
        ("DELETE", "/home") => {
            let device = home
                .profile()
                .as_ref()
                .map(|profile| profile.device_id.clone());
            if let Some(device) = device {
                if let Err(message) = send(
                    node,
                    "DELETE",
                    &format!("/remote/devices/{device}"),
                    None,
                    b"",
                ) {
                    eprintln!(
                        "ob-node: the home node was not told to remove this device: {message}"
                    );
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
                return json_error(
                    "400 Bad Request",
                    "That path cannot be sent to the home node.",
                    origin,
                );
            }
            match send(node, method, target, header(head, "content-type"), body) {
                Ok(answer) => {
                    http_response_cors(&answer.status, &answer.content_type, &answer.body, origin)
                }
                Err(message) => json_body(
                    "502 Bad Gateway",
                    &json!({ "error": message, "code": "home_unreachable" }),
                    origin,
                ),
            }
        }
        _ => json_error("404 Not Found", "not found", origin),
    }
}

/// Sign and send one request to the paired home node. Retries once with
/// the home node's clock when it says ours is off.
fn send(
    node: &Node,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Answer, String> {
    let home = &node.remote.home;
    let (onion, lan, node_key, node_id, signer, discovery) = {
        let profile = home.profile();
        let profile = profile
            .as_ref()
            .ok_or("This node is not paired with a home node.")?;
        (
            profile.onion.clone(),
            profile.lan.clone(),
            profile.node_key,
            profile.node_id.clone(),
            profile.signer(),
            *profile.discovery,
        )
    };
    if let Some(onion) = onion
        .as_deref()
        .filter(|_| !home.discoverable.load(Ordering::Relaxed))
    {
        if let Ok(address) = tormail::OnionAddress::parse(onion) {
            node.remote
                .tor(&node.data_dir)?
                .add_discovery_key(&address, discovery)?;
            home.discoverable.store(true, Ordering::Relaxed);
        }
    }
    let host = onion.clone().unwrap_or_else(|| "home-node".into());
    let route = Route {
        onion: onion.as_deref(),
        lan: &lan,
        node_key: Some(&node_key),
    };
    for attempt in 0..2 {
        let time = now_secs().saturating_add_signed(home.offset.load(Ordering::Relaxed));
        let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        raw.push_str(&signer.headers(&node_id, time, method, path, body));
        if let Some(content_type) = content_type {
            raw.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        raw.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(body);
        let answer = home_link::exchange(node, &route, &raw)?;
        *home.via.lock().unwrap_or_else(|err| err.into_inner()) = Some(answer.via);
        if let Some(next) = answer.next_onion.clone() {
            home.moved(next);
        }
        if attempt == 0 && answer.status.starts_with("401") {
            let reply: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
            if let (Some("clock"), Some(server)) =
                (reply["code"].as_str(), reply["serverTime"].as_u64())
            {
                home.offset
                    .store(server as i64 - now_secs() as i64, Ordering::Relaxed);
                continue;
            }
        }
        return Ok(answer);
    }
    Err("The home node's clock could not be matched.".into())
}
