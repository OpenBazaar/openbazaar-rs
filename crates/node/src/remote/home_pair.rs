//! Pairing this node with a home node: `POST /home/pair` and
//! `GET /home/pair/{requestId}`.
//!
//! This node makes a BIP 340 device key and its own discovery secret (which
//! never leaves it), proves it knows the pairing code, and checks that the
//! four emoji the home node sends match the ones it computes from the code,
//! the home node's key, and its own request. The UI shows them; the owner
//! compares them with the home node's screen and approves there.

use zeroize::Zeroizing;

use serde_json::{json, Value};

use super::auth::Signer;
use super::devices::clean_name;
use super::home::Profile;
use super::home_link::{self, Route};
use super::now_secs;
use super::pairing::{self, Link};
use super::protocol::{decode_base32, pairing_onion, PairRequest};
use crate::http::{json_body, json_error};
use crate::vault_key::random_bytes;
use crate::Node;

pub(crate) struct PendingPair {
    request_id: String,
    link: Link,
    node_key: [u8; 32],
    secret: Zeroizing<[u8; 32]>,
    discovery: Zeroizing<[u8; 32]>,
    name: String,
}

/// The pairing onion, and the LAN addresses once the node key is known (the
/// LAN channel is sealed to it).
fn pairing_route(link: &Link, node_key: Option<&[u8; 32]>) -> (String, Vec<String>) {
    let lan = if node_key.is_some() {
        link.lan.clone()
    } else {
        Vec::new()
    };
    (pairing_onion(&link.secret), lan)
}

fn post(
    node: &Node,
    link: &Link,
    node_key: Option<&[u8; 32]>,
    method: &str,
    path: &str,
    body: &str,
) -> Result<(String, Value), String> {
    let (onion, lan) = pairing_route(link, node_key);
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: {onion}\r\nConnection: close\r\n");
    if !body.is_empty() {
        raw.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    let answer = home_link::exchange(
        node,
        &Route {
            onion: Some(&onion),
            lan: &lan,
            node_key,
        },
        raw.as_bytes(),
    )?;
    Ok((
        answer.status,
        serde_json::from_slice(&answer.body).unwrap_or(Value::Null),
    ))
}

pub(crate) fn start(node: &Node, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The request could not be read.", origin);
    };
    let Some(link) = value["code"].as_str().and_then(pairing::parse) else {
        return json_error(
            "400 Bad Request",
            "That is not a home node pairing code. Paste the link or type the eight words.",
            origin,
        );
    };
    let Some(name) = value["name"].as_str().and_then(clean_name) else {
        return json_error(
            "400 Bad Request",
            "Name this device in 1 to 40 characters.",
            origin,
        );
    };
    let secret = Zeroizing::new(random_bytes::<32>());
    let Some(signer) = Signer::from_secret(&secret) else {
        return json_error(
            "500 Internal Server Error",
            "A device key could not be made. Try again.",
            origin,
        );
    };
    let discovery = Zeroizing::new(random_bytes::<32>());
    let fields = PairRequest {
        key: signer.key().text(),
        watch_key: None,
        discovery_key: Some(tormail::discovery_public(*discovery)),
        name: name.clone(),
        platform: "desktop".into(),
    };
    let request = json!({
        "v": 2,
        "key": fields.key,
        "discoveryKey": fields.discovery_key,
        "name": fields.name,
        "platform": fields.platform,
        "proof": fields.proof(&link.secret),
        "signature": signer.sign(fields.transcript().as_bytes()),
    })
    .to_string();
    let (status, reply) = match post(
        node,
        &link,
        link.node_key.as_ref(),
        "POST",
        "/pair/request",
        &request,
    ) {
        Ok(answer) => answer,
        Err(message) => {
            return json_body(
                "502 Bad Gateway",
                &json!({ "error": message, "code": "home_unreachable" }),
                origin,
            )
        }
    };
    if !status.starts_with("202") {
        let message = reply["error"]
            .as_str()
            .unwrap_or("The home node refused the pairing code.");
        return json_body(
            "400 Bad Request",
            &json!({ "error": message, "code": reply["code"] }),
            origin,
        );
    }
    let node_key: Option<[u8; 32]> = reply["nodeKey"]
        .as_str()
        .and_then(super::auth::decode_hex)
        .and_then(|bytes| bytes.try_into().ok());
    let Some(node_key) =
        node_key.filter(|key| link.node_key.is_none_or(|expected| expected == *key))
    else {
        return json_error(
            "502 Bad Gateway",
            "The home node's key does not match the pairing code.",
            origin,
        );
    };
    let sas = fields.sas(&link.secret, &node_key);
    let theirs: Vec<&str> = reply["sas"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if theirs != sas.iter().map(|(emoji, _)| *emoji).collect::<Vec<_>>() {
        return json_error(
            "502 Bad Gateway",
            "The home node answered with different emoji. Do not approve it; start again.",
            origin,
        );
    }
    let Some(request_id) = reply["requestId"].as_str().map(String::from) else {
        return json_error(
            "502 Bad Gateway",
            "The home node sent no request id.",
            origin,
        );
    };
    *node.remote.home.pending() = Some(PendingPair {
        request_id: request_id.clone(),
        link,
        node_key,
        secret,
        discovery,
        name,
    });
    json_body(
        "202 Accepted",
        &json!({
            "requestId": request_id,
            "sas": sas.iter().map(|(emoji, _)| *emoji).collect::<Vec<_>>(),
            "sasNames": sas.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
        }),
        origin,
    )
}

pub(crate) fn wait(node: &Node, request_id: &str, origin: Option<&str>) -> Vec<u8> {
    let (link, node_key) = {
        let pending = node.remote.home.pending();
        match pending
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
        {
            Some(pending) => (pending.link.clone(), pending.node_key),
            None => {
                return json_error(
                    "404 Not Found",
                    "That pairing request is not waiting here.",
                    origin,
                )
            }
        }
    };
    let reply = match post(
        node,
        &link,
        Some(&node_key),
        "GET",
        &format!("/pair/status/{request_id}"),
        "",
    ) {
        Ok((status, reply)) if status.starts_with("200") => reply,
        Ok((status, _)) => {
            return json_error(
                "502 Bad Gateway",
                &format!("The home node answered {status}."),
                origin,
            )
        }
        Err(message) => {
            return json_body(
                "502 Bad Gateway",
                &json!({ "error": message, "code": "home_unreachable" }),
                origin,
            )
        }
    };
    match reply["state"].as_str() {
        Some("approved") => approved(node, request_id, &reply, origin),
        Some(state @ ("rejected" | "expired")) => {
            node.remote.home.pending().take();
            json_body("200 OK", &json!({ "state": state }), origin)
        }
        _ => json_body("200 OK", &json!({ "state": "pending" }), origin),
    }
}

fn approved(node: &Node, request_id: &str, reply: &Value, origin: Option<&str>) -> Vec<u8> {
    let Some(pending) = node
        .remote
        .home
        .pending()
        .take()
        .filter(|pending| pending.request_id == request_id)
    else {
        return json_error(
            "409 Conflict",
            "That pairing request is no longer waiting here.",
            origin,
        );
    };
    let Some(signer) = Signer::from_secret(&pending.secret) else {
        return json_error(
            "500 Internal Server Error",
            "The device key is gone.",
            origin,
        );
    };
    let node_id = reply["nodeId"].as_str().unwrap_or_default().to_string();
    if reply["deviceId"].as_str() != Some(signer.id().as_str())
        || node_id != super::protocol::node_id(&pending.node_key)
    {
        return json_error(
            "502 Bad Gateway",
            "The home node's answer did not match this device.",
            origin,
        );
    }
    let onion = reply["onion"]
        .as_str()
        .map(String::from)
        .or(pending.link.onion.clone());
    if reply["discoverySecret"]
        .as_str()
        .and_then(decode_base32)
        .is_some()
    {
        eprintln!("ob-node: the home node issued a discovery secret although this node sent its own key; keeping its own");
    }
    let lan = reply["lan"]
        .as_array()
        .map(|lan| {
            lan.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or(pending.link.lan.clone());
    let profile = Profile {
        onion,
        node_id,
        node_key: pending.node_key,
        device_id: signer.id(),
        secret: pending.secret,
        discovery: pending.discovery,
        lan,
        name: pending.name,
        paired_at: now_secs(),
    };
    eprintln!(
        "ob-node: paired with home node {} as device {}",
        profile.node_id, profile.device_id
    );
    match node.remote.home.save(profile) {
        Ok(mut value) => {
            value["state"] = json!("approved");
            json_body("200 OK", &value, origin)
        }
        Err(err) => json_error("500 Internal Server Error", &err.to_string(), origin),
    }
}
