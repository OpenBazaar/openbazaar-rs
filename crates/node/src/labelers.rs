//! Labelers the buyer follows, and the verified labels the node hands its paired client.
//!
//! The follow list is sealed in the vault as `labelers.sealed`: it says what
//! the buyer chose to filter, and it leaves this node only as the labels the
//! paired client asks for. Nothing follows a labeler by default.
//!
//! A labeler's sets come from the built-in sample labeler or from a URL on
//! this machine (`http://127.0.0.1`, `localhost`, or `[::1]`) that serves
//! back-to-back encoded sets. Every set is decoded, verified, and checked to
//! be signed by the key the buyer followed before any of its labels is used.
//! Fetching over Tor is not built. When it is, each fetch must not tell a
//! labeler or a relay which other labelers this node follows.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openbazaar_protocol::{fixture_label_sets, fixture_labeler_key, resolve_labels, LabelSet};
use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "labelers.sealed";
const MAX_FOLLOWS: usize = 32;
const MAX_FETCH: u64 = 4 * 1024 * 1024;
const REFETCH_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
struct Follow {
    key: [u8; 32],
    url: Option<String>,
}

pub(crate) struct Labelers {
    path: PathBuf,
    vault: Vault,
    follows: Vec<Follow>,
    fetched: BTreeMap<[u8; 32], Result<Vec<LabelSet>, String>>,
    fetched_at: Option<Instant>,
}

impl Labelers {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let follows = match vault.read_sealed(&path)? {
            Some(plain) => {
                let value: Value = serde_json::from_slice(&plain).map_err(|err| Error::Detail(format!("{FILE}: {err}")))?;
                parse_follows(&value).map_err(|err| Error::Detail(format!("{FILE}: {err}")))?
            }
            None => Vec::new(),
        };
        Ok(Self {
            path,
            vault,
            follows,
            fetched: BTreeMap::new(),
            fetched_at: None,
        })
    }

    /// Replace the follow list with `{"labelers": [{"key", "url"?}]}` and fetch again.
    pub(crate) fn replace(&mut self, body: &Value) -> Result<Value, String> {
        let follows = parse_follows(body)?;
        let plain = serde_json::to_vec(&follows_value(&follows)).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &plain).map_err(|err| err.to_string())?;
        self.follows = follows;
        self.refresh();
        Ok(self.value())
    }

    /// The follow list, sample labelers, and current labels. Fetches again when the last fetch is stale.
    pub(crate) fn current(&mut self) -> Value {
        if self.fetched_at.is_none_or(|at| at.elapsed() > REFETCH_AFTER) {
            self.refresh();
        }
        self.value()
    }

    fn refresh(&mut self) {
        self.fetched = self.follows.iter().map(|follow| (follow.key, load(follow))).collect();
        self.fetched_at = Some(Instant::now());
    }

    fn value(&self) -> Value {
        let mut verified = Vec::new();
        let follows: Vec<Value> = self
            .follows
            .iter()
            .map(|follow| {
                let (name, sequence, error) = match self.fetched.get(&follow.key) {
                    Some(Ok(sets)) => {
                        verified.extend(sets.iter().cloned());
                        let newest = sets.iter().max_by_key(|set| set.sequence);
                        (newest.map(|set| set.name.clone()), newest.map(|set| set.sequence), None)
                    }
                    Some(Err(message)) => (None, None, Some(message.clone())),
                    None => (None, None, None),
                };
                json!({ "key": hex(&follow.key), "url": follow.url, "name": name, "sequence": sequence, "error": error })
            })
            .collect();
        let labels: Vec<Value> = resolve_labels(&verified)
            .into_iter()
            .map(|label| {
                json!({
                    "labeler": hex(&label.labeler),
                    "labelerName": label.labeler_name,
                    "targetKind": label.target.kind_str(),
                    "target": label.target.to_hex(),
                    "rating": label.label.rating.as_str(),
                    "tags": label.label.tags.names(),
                    "reason": label.reason,
                })
            })
            .collect();
        json!({
            "labelers": follows,
            "samples": [{ "key": hex(&sample_key()), "name": fixture_label_sets()[0].name }],
            "labels": labels,
        })
    }
}

fn sample_key() -> [u8; 32] {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    fixture_labeler_key().x_only_public_key(&secp).0.serialize()
}

/// This follow's sets, each verified and signed by the followed key.
fn load(follow: &Follow) -> Result<Vec<LabelSet>, String> {
    let sets = match &follow.url {
        Some(url) => {
            let bytes = fetch(url)?;
            LabelSet::decode_all(&bytes).map_err(|err| format!("The labeler sent sets that could not be read: {err}."))?
        }
        None if follow.key == sample_key() => fixture_label_sets(),
        None => return Err("No address for this labeler yet. Add the URL it publishes at.".to_string()),
    };
    for set in &sets {
        set.verify().map_err(|err| format!("A label set did not verify: {err}."))?;
        if set.labeler != follow.key {
            return Err("The labeler URL served a set signed by a different key.".to_string());
        }
    }
    Ok(sets)
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(3))
        .redirects(0)
        .build();
    let response = agent.get(url).call().map_err(|_| "The labeler could not be reached.".to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_FETCH + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The labeler could not be read.".to_string())?;
    if bytes.len() as u64 > MAX_FETCH {
        return Err("The labeler sent more than this node reads.".to_string());
    }
    Ok(bytes)
}

fn parse_follows(body: &Value) -> Result<Vec<Follow>, String> {
    let list = body["labelers"].as_array().ok_or("Send {\"labelers\": [...]}.")?;
    if list.len() > MAX_FOLLOWS {
        return Err(format!("Follow at most {MAX_FOLLOWS} labelers."));
    }
    let mut follows: Vec<Follow> = Vec::new();
    for item in list {
        let key = item["key"].as_str().and_then(parse_key).ok_or("A labeler key is 64 hex characters.")?;
        let url = match &item["url"] {
            Value::Null => None,
            Value::String(url) if url.trim().is_empty() => None,
            Value::String(url) if is_local_http(url.trim()) => Some(url.trim().to_string()),
            _ => return Err("A labeler URL must be http on this machine (127.0.0.1, localhost, or [::1]).".to_string()),
        };
        if follows.iter().any(|follow| follow.key == key) {
            continue;
        }
        follows.push(Follow { key, url });
    }
    Ok(follows)
}

fn follows_value(follows: &[Follow]) -> Value {
    json!({ "labelers": follows.iter().map(|follow| json!({ "key": hex(&follow.key), "url": follow.url })).collect::<Vec<_>>() })
}

fn is_local_http(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        Some(_) if authority.starts_with('[') && authority.ends_with(']') => authority,
        Some(_) => return false,
        None => authority,
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

fn parse_key(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    bitcoin::secp256k1::XOnlyPublicKey::from_slice(&out).ok()?;
    Some(out)
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `GET /labelers` and `PUT /labelers`: paired loopback callers only.
pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let mut labelers = node.labelers.lock().unwrap_or_else(|err| err.into_inner());
    if method == "GET" {
        return json_body("200 OK", &labelers.current(), origin);
    }
    if body.len() > 16 * 1024 {
        return json_error("400 Bad Request", "That request is too large.", origin);
    }
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The labeler list could not be read.", origin);
    };
    match labelers.replace(&request) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(message) => json_error("400 Bad Request", &message, origin),
    }
}

#[cfg(test)]
#[path = "labelers_tests.rs"]
mod tests;
