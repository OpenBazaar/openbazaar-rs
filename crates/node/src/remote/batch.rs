//! `POST /remote/batch`: queued mutations in one round trip, idempotent per
//! batch id. See "Batches" in `docs/home-node-protocol.md`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{policy, sync};
use crate::http::{json_body, json_error};
use crate::Node;

pub(crate) const MAX_REQUESTS: usize = 16;
const REMEMBER: Duration = Duration::from_secs(24 * 3600);
const MAX_REMEMBERED: usize = 2048;

#[derive(Default)]
pub(crate) struct Batches {
    done: Mutex<HashMap<(String, String), (Instant, Value)>>,
}

impl Batches {
    fn recall(&self, device: &str, id: &str) -> Option<Value> {
        let mut done = self.done.lock().unwrap_or_else(|err| err.into_inner());
        done.retain(|_, (at, _)| at.elapsed() < REMEMBER);
        done.get(&(device.to_string(), id.to_string()))
            .map(|(_, value)| value.clone())
    }

    fn remember(&self, device: &str, id: &str, value: Value) {
        let mut done = self.done.lock().unwrap_or_else(|err| err.into_inner());
        if done.len() >= MAX_REMEMBERED {
            if let Some(oldest) = done
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(key, _)| key.clone())
            {
                done.remove(&oldest);
            }
        }
        done.insert(
            (device.to_string(), id.to_string()),
            (Instant::now(), value),
        );
    }

    pub(crate) fn forget(&self, device: &str) {
        self.done
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .retain(|(owner, _), _| owner != device);
    }
}

fn is_batch_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn response(
    node: &Node,
    port: u16,
    request: &Value,
    device: Option<&str>,
    via: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    let Some(id) = request["id"].as_str().filter(|id| is_batch_id(id)) else {
        return json_error(
            "400 Bad Request",
            "A batch needs an id of 32 hex digits.",
            origin,
        );
    };
    let Some(requests) = request["requests"]
        .as_array()
        .filter(|list| list.len() <= MAX_REQUESTS)
    else {
        return json_error("400 Bad Request", "A batch holds 1 to 16 requests.", origin);
    };
    let owner = device.unwrap_or("");
    if let Some(answer) = node.remote.batches.recall(owner, id) {
        return json_body("200 OK", &answer, origin);
    }
    let mut results = Vec::new();
    for item in requests {
        let (Some(method), Some(path)) = (item["method"].as_str(), item["path"].as_str()) else {
            results.push(json!({ "status": 400, "body": { "error": "Each request needs a method and a path." } }));
            continue;
        };
        let body = match &item["body"] {
            Value::Null => Vec::new(),
            body => serde_json::to_vec(body).unwrap_or_default(),
        };
        if path.starts_with("/remote/batch") || !path.starts_with('/') {
            results.push(
                json!({ "status": 400, "body": { "error": "That request cannot be batched." } }),
            );
            continue;
        }
        if let Some(message) = device.and(policy::refusal(method, path, &body)) {
            results
                .push(json!({ "status": 403, "body": { "error": message, "code": "node_only" } }));
            continue;
        }
        let (status, body) = sync::run(node, port, method, path, &body, device, via);
        results.push(json!({ "status": status, "body": body }));
        if status >= 500 {
            break;
        }
    }
    let answer = json!({ "id": id, "results": results });
    node.remote.batches.remember(owner, id, answer.clone());
    json_body("200 OK", &answer, origin)
}
