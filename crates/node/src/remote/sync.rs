//! `POST /remote/sync`: everything a device's replica is missing, in one
//! round trip. See "Sync" in `docs/home-node-protocol.md`.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::gateway::loopback_request;
use super::now_secs;
use crate::http::{json_body, Client};
use crate::vault_key::encode_hex;
use crate::Node;

/// Name, source route, and (for lists) the item array and its id field.
const COLLECTIONS: [(&str, &str, Option<(&str, &str)>); 6] = [
    ("catalog", "/catalog", Some(("listings", "hash"))),
    ("orders", "/orders", Some(("orders", "id"))),
    ("stores", "/stores", Some(("stores", "key"))),
    ("wallet", "/wallet", None),
    ("lock", "/lock", None),
    ("network", "/network", None),
];

pub(crate) fn etag(value: &Value) -> String {
    let json = serde_json::to_vec(value).unwrap_or_default();
    encode_hex(&Sha256::digest(json))[..16].to_string()
}

pub(crate) fn response(
    node: &Node,
    port: u16,
    request: &Value,
    device: Option<&str>,
    via: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    let have = &request["have"];
    let mut collections = Map::new();
    for (name, path, items) in COLLECTIONS {
        let (status, value) = run(node, port, "GET", path, b"", device, via);
        let answer = if status >= 400 {
            json!({ "error": { "status": status, "code": value.get("code").cloned().unwrap_or(Value::Null) } })
        } else {
            diff(value, items, &have[name])
        };
        collections.insert(name.to_string(), answer);
    }
    let (_, version) = run(node, port, "GET", "/version", b"", device, via);
    let remote = &node.remote;
    let next = remote
        .devices()
        .moving
        .is_some()
        .then(|| remote.services.onion())
        .flatten();
    let answer = json!({
        "serverTime": now_secs(),
        "node": {
            "nodeId": remote.node_id(),
            "onion": remote.services.onion(),
            "nextOnion": next,
            "lan": remote.lan.addresses(),
            "version": version,
        },
        "collections": collections,
    });
    json_body("200 OK", &answer, origin)
}

/// What changed between the node's `value` and the device's `have`.
pub(crate) fn diff(mut value: Value, items: Option<(&str, &str)>, have: &Value) -> Value {
    let list = items.and_then(|(field, _)| value.as_object_mut()?.remove(field));
    let doc_etag = etag(&value);
    let doc = (have["doc"].as_str() != Some(&doc_etag))
        .then(|| json!({ "etag": doc_etag, "value": value }));
    let Some((_, id_field)) = items else {
        return json!({ "doc": doc });
    };
    let known = have["items"].as_object();
    let mut upsert = Vec::new();
    let mut present = Vec::new();
    for item in list
        .as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = item[id_field].as_str() else {
            continue;
        };
        present.push(id.to_string());
        let item_etag = etag(item);
        if known
            .and_then(|known| known.get(id))
            .and_then(Value::as_str)
            != Some(&item_etag)
        {
            upsert.push(json!({ "id": id, "etag": item_etag, "value": item }));
        }
    }
    let remove: Vec<&String> = known
        .into_iter()
        .flat_map(|known| known.keys())
        .filter(|id| !present.contains(id))
        .collect();
    json!({ "doc": doc, "upsert": upsert, "remove": remove })
}

/// One request through the node's own router; its status and JSON body.
pub(crate) fn run(
    node: &Node,
    port: u16,
    method: &str,
    path: &str,
    body: &[u8],
    device: Option<&str>,
    via: &str,
) -> (u16, Value) {
    let content_type = (!body.is_empty()).then_some("application/json");
    let raw = loopback_request(node, method, path, content_type, body, device, via, port);
    let answer = crate::http::respond(
        node,
        &raw,
        Client {
            loopback: true,
            port,
        },
    );
    split_response(&answer)
}

pub(crate) fn split_response(raw: &[u8]) -> (u16, Value) {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(500);
    let value = serde_json::from_str(body).unwrap_or_else(|_| {
        if body.is_empty() {
            Value::Null
        } else {
            Value::String(body.to_string())
        }
    });
    (status, value)
}
