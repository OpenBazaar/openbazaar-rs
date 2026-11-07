//! Device management on the node's router. The paired local client calls
//! these on loopback; a paired device reaches the same routes through the
//! gateway, which names it in [`REMOTE_DEVICE_HEADER`].
//!
//! | route | answer |
//! |---|---|
//! | `GET /remote` | whether remote access is on, the onion, node id, device count |
//! | `GET /remote/devices` | `{devices, thisDevice}` |
//! | `DELETE /remote/devices/{id}` | removes it and every device it paired |
//! | `POST /remote/pairing` | `{code, expiresInSecs}`: a one-time code for one more device |

use serde_json::{json, Value};

use super::offers::OFFER_LIFETIME;
use super::REMOTE_DEVICE_HEADER;
use crate::http::{json_body, json_error, local_guard, Client};
use crate::http_parse::header;
use crate::Node;

pub(crate) fn response(node: &Node, client: &Client, head: &str, method: &str, path: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let this = header(head, REMOTE_DEVICE_HEADER);
    match (method, path) {
        ("GET", "/remote") => json_body("200 OK", &status(node, this), origin),
        ("GET", "/remote/devices") => json_body("200 OK", &devices(node, this), origin),
        ("DELETE", path) if path.starts_with("/remote/devices/") => {
            let id = &path["/remote/devices/".len()..];
            let gone = match node.remote.devices().revoke(id) {
                Ok(gone) => gone,
                Err(err) => {
                    eprintln!("ob-node: remote devices: {err}");
                    return json_error("500 Internal Server Error", "The device list could not be saved.", origin);
                }
            };
            if gone.is_empty() {
                return json_error("404 Not Found", "That device is not paired with this node.", origin);
            }
            node.remote.offers().drop_from(&gone);
            for id in &gone {
                node.remote.replay.forget(id);
            }
            eprintln!("ob-node: removed device(s) {}", gone.join(", "));
            let mut value = devices(node, this);
            value["removed"] = json!(gone);
            json_body("200 OK", &value, origin)
        }
        ("POST", "/remote/pairing") => match node.remote.issue_code(this.map(String::from)) {
            Some(code) => json_body("200 OK", &json!({ "code": code, "expiresInSecs": OFFER_LIFETIME.as_secs() }), origin),
            None => json_error(
                "409 Conflict",
                "Remote access is off on this node. Start it with ob-node serve --remote.",
                origin,
            ),
        },
        _ => json_error("404 Not Found", "not found", origin),
    }
}

fn status(node: &Node, this: Option<&str>) -> Value {
    let remote = &node.remote;
    let onion = remote.onion.get();
    json!({
        "enabled": remote.address.get().is_some(),
        "onion": onion.and_then(|o| o.onion_address()).map(|o| o.to_string()),
        "onionState": onion.map(|o| o.state()),
        "nodeId": remote.node_id(),
        "devices": remote.devices().list().len(),
        "webShop": remote.web_dir().is_some(),
        "viaDevice": this,
    })
}

fn devices(node: &Node, this: Option<&str>) -> Value {
    let devices = node.remote.devices();
    json!({
        "devices": devices.list().iter().map(|device| device.value(this)).collect::<Vec<_>>(),
        "thisDevice": this,
    })
}
