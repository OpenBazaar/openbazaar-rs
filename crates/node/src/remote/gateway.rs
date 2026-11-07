//! The remote gateway: where onion streams land.
//!
//! It listens on loopback only; the onion service pipes its streams here
//! byte for byte, and Tor has already encrypted the connection and proved
//! the onion key to the caller. This listener then decides who the caller
//! is:
//!
//! * `GET /remote/hello` and `POST /remote/pair` (with a one-time code)
//!   need no signature.
//! * An unsigned `GET` for a file of the web export gets the file.
//! * Everything else must be signed by a paired device ([`super::auth`]).
//!   A signed request passes [`super::policy`], then runs through the
//!   node's own router as the paired local client, one request at a time
//!   with the loopback server.
//!
//! The Host must be this node's onion or this listener's own loopback
//! address, and a browser `Origin` must match it.

use std::io::{ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::auth::{parse_key, pair_digest, verify, Refused, Signed};
use super::devices::clean_name;
use super::offers::Taken;
use super::{now_secs, policy, web, REMOTE_DEVICE_HEADER};
use crate::http::{http_response_cors, json_body, json_error};
use crate::http_parse::{self, header};
use crate::Node;

const MAX_CONNECTIONS: usize = 16;
const MAX_PAIR_BODY: usize = 1024;

pub(crate) fn serve(node: &Node, listener: TcpListener, main_port: u16, stop: &AtomicBool) {
    if let Err(err) = listener.set_nonblocking(true) {
        eprintln!("ob-node: remote gateway: {err}");
        return;
    }
    let active = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                        continue;
                    }
                    active.fetch_add(1, Ordering::Relaxed);
                    let active = &active;
                    scope.spawn(move || {
                        connection(node, stream, main_port);
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(err) => {
                    eprintln!("ob-node: remote gateway: {err}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    });
}

fn connection(node: &Node, mut stream: TcpStream, main_port: u16) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let raw = match http_parse::read(&mut stream) {
        Ok(raw) => raw,
        Err(_) => return,
    };
    let response = catch_unwind(AssertUnwindSafe(|| respond(node, &raw, main_port))).unwrap_or_else(|_| {
        eprintln!("ob-node: a remote request handler panicked");
        http_response_cors("500 Internal Server Error", "text/plain", b"internal error", None)
    });
    let _ = stream.write_all(&response);
}

pub(crate) fn respond(node: &Node, raw: &[u8], main_port: u16) -> Vec<u8> {
    let request = match http_parse::parse(raw) {
        Ok(request) => request,
        Err(refusal) => return http_response_cors(refusal.status, "text/plain", refusal.message.as_bytes(), None),
    };
    let Some(host) = header(request.head, "host").map(str::to_ascii_lowercase) else {
        return misdirected();
    };
    if !node.remote.serves_host(&host) {
        return misdirected();
    }
    if let Some(origin) = header(request.head, "origin") {
        if origin != format!("http://{host}") {
            return json_error("403 Forbidden", "That page may not call this node.", None);
        }
    }
    let (method, path, body) = (request.method, request.path, request.body);
    let signed = Signed::read(request.head);
    match (method, path) {
        ("GET", "/remote/hello") => json_body("200 OK", &json!({ "service": "openbazaar-home-node", "nodeId": node.remote.node_id() }), None),
        ("POST", "/remote/pair") => pair(node, body),
        ("GET", _) if signed.is_none() => match node.remote.web_dir().and_then(|dir| web::response(dir, path)) {
            Some(file) => file,
            None => unauthorized("unsigned", "Pair this device with the node first."),
        },
        _ => match signed {
            Some(signed) => signed_request(node, request.head, &signed, method, path, body, main_port),
            None => unauthorized("unsigned", "Pair this device with the node first."),
        },
    }
}

fn signed_request(node: &Node, head: &str, signed: &Signed<'_>, method: &str, path: &str, body: &[u8], main_port: u16) -> Vec<u8> {
    let now = now_secs();
    {
        let mut devices = node.remote.devices();
        let Some(device) = devices.get(signed.device) else {
            return unauthorized("unknown_device", "This device is not paired with the node, or was removed.");
        };
        if !signed.verifies(&device.key, &devices.node_id, method, path, body) {
            return unauthorized("unsigned", "The request signature did not verify.");
        }
        match node.remote.replay.admit(signed, now) {
            Ok(()) => {}
            Err(Refused::Clock) => {
                return json_body(
                    "401 Unauthorized",
                    &json!({ "error": "This device's clock is too far from the node's.", "code": "clock", "serverTime": now }),
                    None,
                )
            }
            Err(_) => return unauthorized("replay", "That request was already used."),
        }
        devices.touch(signed.device, now);
    }
    if let Some(message) = policy::refusal(method, path, body) {
        return json_body("403 Forbidden", &json!({ "error": message, "code": "node_only" }), None);
    }
    let mut raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{main_port}\r\nX-Ob-Pair: {}\r\n{REMOTE_DEVICE_HEADER}: {}\r\n",
        node.pairing_token(),
        signed.device
    );
    if let Some(content_type) = header(head, "content-type") {
        raw.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    raw.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut raw = raw.into_bytes();
    raw.extend_from_slice(body);
    let _one = super::dispatch_lock();
    crate::http::respond(node, &raw, crate::http::Client { loopback: true, port: main_port })
}

/// `POST /remote/pair {secret, publicKey, name, signature}`.
fn pair(node: &Node, body: &[u8]) -> Vec<u8> {
    if body.len() > MAX_PAIR_BODY {
        return json_error("400 Bad Request", "That request is too large.", None);
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The request could not be read.", None);
    };
    let text = |name: &str| value.get(name).and_then(Value::as_str).unwrap_or_default();
    let (secret, key_hex, raw_name, signature) = (text("secret"), text("publicKey"), text("name"), text("signature"));
    let Some(name) = clean_name(raw_name) else {
        return json_error("400 Bad Request", "Name this device in 1 to 40 characters.", None);
    };
    let Some(key) = parse_key(key_hex) else {
        return json_error("400 Bad Request", "The device key could not be read.", None);
    };
    if !verify(&key, pair_digest(secret, key_hex, raw_name), signature) {
        return json_error("400 Bad Request", "The device did not prove it holds its key.", None);
    }
    let by = match node.remote.offers().take(secret, Instant::now()) {
        Taken::Paired(by) => by,
        Taken::Wrong => return json_error("401 Unauthorized", "That pairing code is wrong, used, or expired.", None),
        Taken::Paused => {
            return json_error("429 Too Many Requests", "Too many wrong pairing codes. Try again in a few minutes.", None)
        }
    };
    let mut devices = node.remote.devices();
    if by.as_deref().is_some_and(|by| devices.get(by).is_none()) {
        return json_error("401 Unauthorized", "The device that made this code was removed.", None);
    }
    match devices.add(key, &name, by, now_secs()) {
        Ok(device) => {
            eprintln!("ob-node: paired device {:?} ({})", device.name, device.id);
            json_body("200 OK", &json!({ "nodeId": devices.node_id, "deviceId": device.id, "name": device.name }), None)
        }
        Err(message) => json_error("409 Conflict", &message, None),
    }
}

fn unauthorized(code: &str, message: &str) -> Vec<u8> {
    json_body("401 Unauthorized", &json!({ "error": message, "code": code }), None)
}

fn misdirected() -> Vec<u8> {
    http_response_cors("421 Misdirected Request", "text/plain", b"This is not the address of this node.", None)
}
