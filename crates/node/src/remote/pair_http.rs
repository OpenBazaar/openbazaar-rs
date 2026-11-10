//! `POST /pair/request`, `GET /pair/status/{id}`, and the owner's decision.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::devices::{clean_name, NewDevice, PLATFORMS};
use super::gateway::Entry;
use super::pairing::{Refusal, Request, State};
use super::protocol::{base32, decode_base32, DeviceKey, PairRequest};
use super::{now_secs, pair_cli, pair_log};
use crate::http::{json_body, json_error};
use crate::vault_key::{encode_hex, random_bytes};
use crate::Node;

const MAX_BODY: usize = 2048;
const STATUS_WAIT: Duration = Duration::from_secs(25);

pub(crate) fn response(node: &Node, entry: Entry, method: &str, path: &str, body: &[u8]) -> Option<Vec<u8>> {
    // Where the request came in, for the log.
    let arrived = match entry {
        Entry::Pairing => "a pairing onion",
        Entry::Lan => "the local network",
        Entry::Admin | Entry::Web => "the admin gateway",
    };
    match (method, path) {
        ("POST", "/pair/request") => {
            let answer = request(node, arrived, body);
            if !answer.starts_with(b"HTTP/1.1 202") {
                pair_log::refused(arrived, &answer);
            }
            Some(answer)
        }
        ("GET", path) => path
            .strip_prefix("/pair/status/")
            .map(|id| status(node, id)),
        _ => None,
    }
}

fn is_discovery_key(text: &str) -> bool {
    text.strip_prefix("descriptor:x25519:")
        .and_then(decode_base32)
        .is_some_and(|bytes| bytes.len() == 32)
}

fn request(node: &Node, arrived: &str, body: &[u8]) -> Vec<u8> {
    if !node.remote.enabled() {
        return json_error("409 Conflict", "Remote access is off on this node.", None);
    }
    if body.len() > MAX_BODY {
        return json_error("400 Bad Request", "That request is too large.", None);
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The request could not be read.", None);
    };
    let text = |name: &str| value.get(name).and_then(Value::as_str);
    if value.get("v").and_then(Value::as_u64) != Some(2) {
        return json_error(
            "400 Bad Request",
            "This app speaks a different pairing version. Update it.",
            None,
        );
    }
    let Some(key) = text("key").and_then(DeviceKey::parse) else {
        return json_error("400 Bad Request", "The device key could not be read.", None);
    };
    let watch = match text("watchKey") {
        Some(watch) => match DeviceKey::parse(watch) {
            Some(watch) if watch != key => Some(watch),
            _ => return json_error("400 Bad Request", "The watch key could not be read.", None),
        },
        None => None,
    };
    let discovery = text("discoveryKey").map(String::from);
    if discovery
        .as_deref()
        .is_some_and(|key| !is_discovery_key(key))
    {
        return json_error(
            "400 Bad Request",
            "The discovery key must look like descriptor:x25519:….",
            None,
        );
    }
    let raw_name = text("name").unwrap_or_default();
    if clean_name(raw_name).as_deref() != Some(raw_name) {
        return json_error(
            "400 Bad Request",
            "Name this device in 1 to 40 characters.",
            None,
        );
    }
    let platform = text("platform").unwrap_or_default();
    if !PLATFORMS.contains(&platform) {
        return json_error(
            "400 Bad Request",
            "The platform must be ios, android, desktop, or browser.",
            None,
        );
    }
    let fields = PairRequest {
        key: key.text(),
        watch_key: watch.as_ref().map(DeviceKey::text),
        discovery_key: discovery,
        name: raw_name.to_string(),
        platform: platform.to_string(),
    };
    if !key.verifies(
        fields.transcript().as_bytes(),
        text("signature").unwrap_or_default(),
    ) {
        return json_error(
            "400 Bad Request",
            "The device did not prove it holds its key.",
            None,
        );
    }
    let request = match node.remote.pairing().request(
        fields,
        text("proof").unwrap_or_default(),
        Instant::now(),
    ) {
        Ok(request) => request,
        Err(Refusal::Wrong) => {
            return coded(
                "401 Unauthorized",
                "wrong_code",
                "That pairing code is wrong, used, or expired.",
            )
        }
        Err(Refusal::Taken) => {
            return coded(
                "409 Conflict",
                "code_taken",
                "Another device is already pairing with this code.",
            )
        }
        Err(Refusal::Paused) => {
            return coded(
                "429 Too Many Requests",
                "paused",
                "Too many wrong pairing codes. Try again in a few minutes.",
            )
        }
    };
    let (sas, names) = sas(node, &request);
    pair_log::line(format!(
        "code {}: a request from {:?} ({}) arrived by {arrived}; it waits for the owner",
        request.code, request.fields.name, request.fields.platform
    ));
    pair_cli::announce_request(&request.fields.name, &request.fields.platform, &sas, &names);
    json_body(
        "202 Accepted",
        &json!({
            "requestId": request.id,
            "nodeKey": encode_hex(&node.remote.devices().node_public),
            "sas": sas,
            "sasNames": names,
            "expiresInSecs": super::pairing::CODE_LIFETIME.as_secs(),
        }),
        None,
    )
}

pub(crate) fn sas(node: &Node, request: &Request) -> (Vec<&'static str>, Vec<&'static str>) {
    let node_public = node.remote.devices().node_public;
    request
        .fields
        .sas(&request.secret, &node_public)
        .iter()
        .map(|(emoji, name)| (*emoji, *name))
        .unzip()
}

fn status(node: &Node, id: &str) -> Vec<u8> {
    let started = Instant::now();
    loop {
        let state = node
            .remote
            .pairing()
            .get(id, Instant::now())
            .map(|request| request.state);
        let answer = match state {
            None => json!({ "state": "expired" }),
            Some(State::Rejected) => json!({ "state": "rejected" }),
            Some(State::Approved(result)) => result,
            Some(State::Pending) if started.elapsed() < STATUS_WAIT => {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            Some(State::Pending) => json!({ "state": "pending" }),
        };
        return json_body("200 OK", &answer, None);
    }
}

/// Approve or reject a pending request. `by` is the device deciding, or
/// `None` for the node itself.
pub(crate) fn decide(node: &Node, id: &str, approve: bool) -> Result<Option<Value>, String> {
    let remote = &node.remote;
    let Some(request) = remote
        .pairing()
        .get(id, Instant::now())
        .filter(|request| request.state == State::Pending)
    else {
        return Err("That pairing request expired or was already decided.".into());
    };
    let from = format!("code {}: the request from {:?}", request.code, request.fields.name);
    if !approve {
        remote.pairing().decide(id, State::Rejected, Instant::now());
        pair_log::line(format!("{from} was rejected"));
        return Ok(None);
    }
    if request
        .by
        .as_deref()
        .is_some_and(|by| remote.devices().get(by).is_none())
    {
        remote.pairing().decide(id, State::Rejected, Instant::now());
        pair_log::line(format!("{from} was rejected: the device that made the code was removed"));
        return Err("The device that made this code was removed.".into());
    }
    let issued = request
        .fields
        .discovery_key
        .is_none()
        .then(random_bytes::<32>);
    let fields = &request.fields;
    let new = NewDevice {
        key: DeviceKey::parse(&fields.key).ok_or("The device key could not be read.")?,
        watch: fields.watch_key.as_deref().and_then(DeviceKey::parse),
        discovery: fields
            .discovery_key
            .clone()
            .or_else(|| issued.map(tormail::discovery_public)),
        name: fields.name.clone(),
        platform: fields.platform.clone(),
        paired_by: request.by.clone(),
    };
    let device = remote.devices().add(new, now_secs())?;
    remote.refresh_discovery();
    eprintln!(
        "ob-node: paired device {:?} ({}, {})",
        device.name, device.platform, device.id
    );
    pair_log::line(format!("{from} was approved as device {}", device.id));
    let result = json!({
        "state": "approved",
        "deviceId": device.id,
        "nodeId": remote.node_id(),
        "onion": remote.services.onion(),
        "discoverySecret": issued.map(|secret| base32(&secret)),
        "lan": remote.lan.addresses(),
    });
    remote
        .pairing()
        .decide(id, State::Approved(result), Instant::now());
    Ok(Some(device.value(None)))
}

fn coded(status: &str, code: &str, message: &str) -> Vec<u8> {
    json_body(status, &json!({ "error": message, "code": code }), None)
}
