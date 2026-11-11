//! Remote-access management on the node's router. The paired local client
//! calls these on loopback; a paired device reaches the same routes through
//! the gateway, which names it in [`REMOTE_DEVICE_HEADER`].
//!
//! | route | answer |
//! |---|---|
//! | `GET /remote` | whether remote access is on, the onion, LAN addresses, node id, counts, and `tor`: the health of the node's Tor |
//! | `GET /remote/devices` | `{devices, thisDevice}` |
//! | `DELETE /remote/devices/{id}` | removes it and every device it paired; `{"rotate": true}` also moves the onion |
//! | `POST /remote/pairing` | a one-time code for one more device: words, link, QR |
//! | `GET /remote/pending`, `POST /remote/pending/{id}` | pairing requests waiting for the owner, and how each live code's onion is doing |
//! | `POST /remote/browser-credential` | a ready-made device for Tor Browser |
//! | `GET /remote/passkeys`, `POST /remote/passkeys`, `DELETE /remote/passkeys/{id}` | see [`super::passkeys`] |
//! | `POST /remote/rotate` | move the admin onion to a fresh address |
//! | `POST /remote/lan-page` | show the LAN setup page for 10 minutes (the node's own client only) |
//! | `POST /remote/sync`, `POST /remote/batch` | see [`super::sync`], [`super::batch`] |
//! | `PUT /remote/push`, `DELETE /remote/push` | the calling device's push token |

use std::time::Instant;

use serde_json::{json, Value};

use super::devices::{clean_name, NewDevice, Push};
use super::pair_ready::Onion;
use super::protocol::{base32, words, DeviceKey};
use super::{
    batch, pair_cli, pair_http, passkeys, push, services, sync, REMOTE_DEVICE_HEADER,
    REMOTE_VIA_HEADER,
};
use crate::http::{json_body, json_error, local_guard, Client};
use crate::http_parse::header;
use crate::vault_key::random_bytes;
use crate::Node;

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
    let this = header(head, REMOTE_DEVICE_HEADER);
    let via = header(head, REMOTE_VIA_HEADER).unwrap_or("local");
    let ok = |value: Value| json_body("200 OK", &value, origin);
    let fail = |status: &str, message: &str| json_error(status, message, origin);
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    match (method, path) {
        ("GET", "/remote") => ok(status(node, this)),
        ("GET", "/remote/devices") => ok(devices(node, this)),
        ("DELETE", path) if path.starts_with("/remote/devices/") => {
            let id = &path["/remote/devices/".len()..];
            match node.remote.revoke(id) {
                Ok(gone) if gone.is_empty() => {
                    fail("404 Not Found", "That device is not paired with this node.")
                }
                Ok(gone) => {
                    eprintln!("ob-node: removed device(s) {}", gone.join(", "));
                    let mut answer = devices(node, this);
                    answer["removed"] = json!(gone);
                    if value["rotate"] == json!(true) {
                        answer["onion"] = json!(services::rotate(node).ok());
                    }
                    ok(answer)
                }
                Err(err) => {
                    eprintln!("ob-node: remote devices: {err}");
                    fail(
                        "500 Internal Server Error",
                        "The device list could not be saved.",
                    )
                }
            }
        }
        ("POST", "/remote/pairing") => match node.remote.issue_code(this.map(String::from)) {
            Some(code) => {
                let link = node.remote.link_for(&code);
                ok(json!({
                    "codeId": code.id,
                    "words": words(&code.secret),
                    "link": link,
                    "qrSvg": pair_cli::qr_svg(&link),
                    "expiresInSecs": code.expires_in(Instant::now()).as_secs(),
                    // The page holds the QR while the onion is "publishing",
                    // unless the link also works on the local network.
                    "onion": code.onion(Instant::now()).map(Onion::name),
                    "lan": !node.remote.lan.addresses().is_empty(),
                }))
            }
            None => fail(
                "409 Conflict",
                "Remote access is off on this node. Start it with ob-node serve --remote.",
            ),
        },
        ("GET", "/remote/pending") => ok(pending(node)),
        ("POST", path) if path.starts_with("/remote/pending/") => {
            let id = &path["/remote/pending/".len()..];
            match pair_http::decide(node, id, value["approve"] == json!(true)) {
                Ok(device) => ok(json!({ "device": device })),
                Err(message) => fail("409 Conflict", &message),
            }
        }
        ("POST", "/remote/browser-credential") => browser_credential(node, this, &value, origin),
        ("GET", "/remote/passkeys") => ok(passkeys::list(node)),
        ("POST", "/remote/passkeys") => match passkeys::add(node, &value) {
            Ok(list) => ok(list),
            Err(message) => fail("409 Conflict", &message),
        },
        ("DELETE", path) if path.starts_with("/remote/passkeys/") => {
            match passkeys::remove(node, &path["/remote/passkeys/".len()..]) {
                Ok(Some(list)) => ok(list),
                Ok(None) => fail("404 Not Found", "This node has no such passkey."),
                Err(message) => fail("500 Internal Server Error", &message),
            }
        }
        ("POST", "/remote/rotate") => match services::rotate(node) {
            Ok(onion) => ok(json!({ "nextOnion": onion })),
            Err(message) => fail("409 Conflict", &message),
        },
        ("POST", "/remote/lan-page") if this.is_none() => {
            node.remote.lan.show_page_for_a_while();
            ok(json!({ "lan": node.remote.lan.addresses() }))
        }
        ("POST", "/remote/sync") => sync::response(node, client.port, &value, this, via, origin),
        ("POST", "/remote/batch") => batch::response(node, client.port, &value, this, via, origin),
        ("PUT" | "DELETE", "/remote/push") => match this {
            Some(device) => {
                let push = (method == "PUT")
                    .then(|| push::parse_token(&value))
                    .flatten();
                if method == "PUT" && push.is_none() {
                    return fail("400 Bad Request", "Send {platform: apns or fcm, token}.");
                }
                set_push(node, device, push, origin)
            }
            None => fail(
                "400 Bad Request",
                "Push is set by a paired device for itself.",
            ),
        },
        _ => fail("404 Not Found", "not found"),
    }
}

fn status(node: &Node, this: Option<&str>) -> Value {
    let remote = &node.remote;
    let devices = remote.devices();
    let mut body = json!({
        "enabled": remote.enabled(),
        "onion": remote.services.onion(),
        "onionState": remote.services.state(),
        "moving": devices.moving.is_some(),
        "lan": remote.lan.addresses(),
        "nodeId": devices.node_id,
        "devices": devices.list().len(),
        "pending": remote.pairing().pending(Instant::now()).len(),
        "webShop": remote.web_dir().is_some(),
        "push": remote.config().push_gateway.is_some(),
        "viaDevice": this,
    });
    if let Some(onion) = remote.services.sale_onion() {
        body["saleOnion"] = json!(onion);
    }
    // How Tor is doing, on a node that runs it: see `tor_health`.
    if let Some(tor) = remote.tor_health() {
        body["tor"] = tor;
    }
    body
}

fn devices(node: &Node, this: Option<&str>) -> Value {
    let devices = node.remote.devices();
    json!({
        "devices": devices.list().iter().map(|device| device.value(this)).collect::<Vec<_>>(),
        "thisDevice": this,
    })
}

fn pending(node: &Node) -> Value {
    let now = Instant::now();
    let list = node.remote.pairing().pending(now);
    let items: Vec<Value> = list
        .iter()
        .map(|(request, age)| {
            let (sas, names) = pair_http::sas(node, request);
            json!({
                "requestId": request.id,
                "name": request.fields.name,
                "platform": request.fields.platform,
                "deviceId": DeviceKey::parse(&request.fields.key).map(|key| key.id()),
                "sas": sas,
                "sasNames": names,
                "ageSecs": age.as_secs(),
            })
        })
        .collect();
    let codes: Vec<Value> = node
        .remote
        .pairing()
        .live(now)
        .iter()
        .map(|code| {
            json!({
                "codeId": code.id,
                "onion": code.onion(now).map(Onion::name),
                "expiresInSecs": code.expires_in(now).as_secs(),
            })
        })
        .collect();
    json!({ "pending": items, "codes": codes })
}

/// A device for Tor Browser, approved by the device or node that asks: a
/// BIP 340 access key and a discovery secret, both shown once.
fn browser_credential(
    node: &Node,
    this: Option<&str>,
    value: &Value,
    origin: Option<&str>,
) -> Vec<u8> {
    let Some(name) = value["name"].as_str().and_then(clean_name) else {
        return json_error(
            "400 Bad Request",
            "Name this browser in 1 to 40 characters.",
            origin,
        );
    };
    let secret = random_bytes::<32>();
    let Some(signer) = super::auth::Signer::from_secret(&secret) else {
        return json_error("500 Internal Server Error", "Try again.", origin);
    };
    let discovery = random_bytes::<32>();
    let new = NewDevice {
        key: signer.key(),
        watch: None,
        discovery: Some(tormail::discovery_public(discovery)),
        name,
        platform: "browser".into(),
        paired_by: this.map(String::from),
    };
    let added = node.remote.devices().add(new, super::now_secs());
    if let Err(message) = added {
        return json_error("409 Conflict", &message, origin);
    }
    node.remote.refresh_discovery();
    json_body(
        "200 OK",
        &json!({
            "deviceId": signer.id(),
            "accessKey": format!("obkey-{}", base32(&secret)),
            "discoverySecret": base32(&discovery),
            "onion": node.remote.services.onion(),
        }),
        origin,
    )
}

fn set_push(node: &Node, device: &str, push: Option<Push>, origin: Option<&str>) -> Vec<u8> {
    let saved = node.remote.devices().set_push(device, push);
    match saved {
        Ok(()) => json_body("200 OK", &devices(node, Some(device)), origin),
        Err(err) => {
            eprintln!("ob-node: remote devices: {err}");
            json_error(
                "500 Internal Server Error",
                "The device list could not be saved.",
                origin,
            )
        }
    }
}
