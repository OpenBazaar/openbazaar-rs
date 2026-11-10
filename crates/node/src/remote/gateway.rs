//! The remote gateways: where onion streams and LAN requests land.
//!
//! Both listen on loopback only. The admin onion pipes its streams to the
//! admin gateway and each pairing onion to the pairing gateway, byte for
//! byte; the LAN channel hands over the request it decrypted. Tor (or the
//! LAN channel) has already encrypted the connection and proved the node's
//! key to the caller. The gateway then decides who the caller is:
//!
//! * `/pair/request` and `/pair/status/{id}` need no signature (the pairing
//!   proof does that work). They are all the pairing gateway serves.
//! * `GET /remote/hello`, and unsigned `GET`s for files of the web export.
//! * Everything else must be signed by a paired device ([`super::auth`]),
//!   pass [`super::policy`], and then runs through the node's own router as
//!   the paired local client, one request at a time with the loopback server.
//!
//! On the admin gateway the Host must be one of this node's admin onions or
//! the gateway's own loopback address, and a browser `Origin` must match it.
//! The web gateway ([`super::web_origin`]) is the admin gateway at one
//! `https://` address, without the pairing routes.

use std::io::{ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;

use super::auth::{Refused, Signed};
use super::{
    now_secs, pair_http, passkeys, policy, web, NEXT_ONION_HEADER, REMOTE_DEVICE_HEADER,
    REMOTE_VIA_HEADER,
};
use crate::http::{http_response_cors, json_body, json_error};
use crate::http_parse::{self, header};
use crate::Node;

const MAX_CONNECTIONS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Admin,
    Pairing,
    Lan,
    Web,
}

pub(crate) fn serve(
    node: &Node,
    listener: TcpListener,
    entry: Entry,
    main_port: u16,
    stop: &AtomicBool,
) {
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
                        connection(node, stream, entry, main_port);
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(err) => {
                    eprintln!("ob-node: remote gateway: {err}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    });
}

fn connection(node: &Node, mut stream: TcpStream, entry: Entry, main_port: u16) {
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let Ok(raw) = http_parse::read(&mut stream) else {
        return;
    };
    let _ = stream.write_all(&answer(node, &raw, entry, main_port));
}

/// [`respond`], with a handler panic turned into a bare 500.
pub(crate) fn answer(node: &Node, raw: &[u8], entry: Entry, main_port: u16) -> Vec<u8> {
    catch_unwind(AssertUnwindSafe(|| respond(node, raw, entry, main_port))).unwrap_or_else(|_| {
        eprintln!("ob-node: a remote request handler panicked");
        http_response_cors(
            "500 Internal Server Error",
            "text/plain",
            b"internal error",
            None,
        )
    })
}

pub(crate) fn respond(node: &Node, raw: &[u8], entry: Entry, main_port: u16) -> Vec<u8> {
    let request = match http_parse::parse(raw) {
        Ok(request) => request,
        Err(refusal) => {
            return http_response_cors(
                refusal.status,
                "text/plain",
                refusal.message.as_bytes(),
                None,
            )
        }
    };
    let (method, path, body) = (request.method, request.path, request.body);
    if entry != Entry::Web {
        if let Some(answer) = pair_http::response(node, entry, method, path, body) {
            return answer;
        }
    }
    let via = match entry {
        Entry::Pairing => {
            return json_error("404 Not Found", "This address only pairs devices.", None)
        }
        Entry::Lan => "lan".to_string(),
        Entry::Web => {
            let Some(origin) = node.remote.web_origin() else {
                return misdirected();
            };
            if let Some(refusal) = origin.refusal(method, request.head) {
                return refusal;
            }
            if let Some(answer) = passkeys::public_response(node, origin, method, path, body) {
                return answer;
            }
            "web".to_string()
        }
        Entry::Admin => {
            let Some(host) = header(request.head, "host").map(str::to_ascii_lowercase) else {
                return misdirected();
            };
            let Some(via) = admin_host(node, &host) else {
                return misdirected();
            };
            if header(request.head, "origin")
                .is_some_and(|origin| origin != format!("http://{host}"))
            {
                return json_error("403 Forbidden", "That page may not call this node.", None);
            }
            via
        }
    };
    let signed = Signed::read(request.head);
    match (method, path, signed) {
        ("GET", "/remote/hello", _) => {
            let mut hello = json!({ "service": "openbazaar-home-node", "nodeId": node.remote.node_id() });
            if entry == Entry::Web {
                // The sign-in page offers a passkey only when the node has one,
                // and offers to create the first on a node nobody owns yet.
                hello["passkeys"] = json!(!node.remote.devices().passkeys.is_empty());
                hello["claimable"] = json!(passkeys::claimable(node));
            }
            json_body("200 OK", &hello, None)
        }
        (_, _, Some(signed)) => signed_request(
            node,
            request.head,
            &signed,
            &via,
            method,
            path,
            body,
            main_port,
        ),
        ("GET", _, None) if matches!(entry, Entry::Admin | Entry::Web) => match node
            .remote
            .web_dir()
            .and_then(|dir| web::response(dir, path))
        {
            Some(file) => file,
            None => unauthorized("unsigned", "Pair this device with the node first."),
        },
        _ => unauthorized("unsigned", "Pair this device with the node first."),
    }
}

/// `tor:{generation}` for an admin onion, `loopback` for the gateway's own address.
fn admin_host(node: &Node, host: &str) -> Option<String> {
    if let Some(generation) = node.remote.services.generation_of(host) {
        return Some(format!("tor:{generation}"));
    }
    node.remote
        .gateway
        .get()
        .filter(|addr| host == addr.to_string() || host == format!("localhost:{}", addr.port()))
        .map(|_| "loopback".to_string())
}

#[allow(clippy::too_many_arguments)]
fn signed_request(
    node: &Node,
    head: &str,
    signed: &Signed<'_>,
    via: &str,
    method: &str,
    path: &str,
    body: &[u8],
    main_port: u16,
) -> Vec<u8> {
    let now = now_secs();
    let target = super::request_target(head).unwrap_or(path);
    let device = {
        let mut devices = node.remote.devices();
        let Some((device, watch)) = devices.signer(signed.device) else {
            return unauthorized(
                "unknown_device",
                "This device is not paired with the node, or was removed.",
            );
        };
        let key = if watch {
            device.watch.clone()
        } else {
            Some(device.key.clone())
        };
        let device_id = device.id.clone();
        if !key.is_some_and(|key| signed.verifies(&key, &devices.node_id, method, target, body)) {
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
            Err(Refused::Replay) => {
                return unauthorized("replay", "That request was already used.")
            }
        }
        if watch && (method, path) != ("POST", "/remote/sync") {
            return json_body(
                "403 Forbidden",
                &json!({ "error": "A watch key may only sync.", "code": "watch_only" }),
                None,
            );
        }
        let generation = via
            .strip_prefix("tor:")
            .and_then(|generation| generation.parse().ok());
        devices.touch(
            &device_id,
            via.split(':').next().unwrap_or(via),
            generation,
            now,
        );
        device_id
    };
    if let Some(message) = policy::refusal(method, path, body, node.lock.backup_confirmed()) {
        return json_body(
            "403 Forbidden",
            &json!({ "error": message, "code": "node_only" }),
            None,
        );
    }
    let raw = loopback_request(
        node,
        method,
        target,
        header(head, "content-type"),
        body,
        Some(&device),
        via,
        main_port,
    );
    let mut answer = {
        let _one = super::dispatch_lock();
        crate::http::respond(
            node,
            &raw,
            crate::http::Client {
                loopback: true,
                port: main_port,
            },
        )
    };
    if node.remote.devices().moving.is_some() {
        if let (Some(next), Some(end)) = (
            node.remote.services.onion(),
            answer.windows(4).position(|window| window == b"\r\n\r\n"),
        ) {
            answer.splice(
                end + 2..end + 2,
                format!("{NEXT_ONION_HEADER}: {next}\r\n").into_bytes(),
            );
        }
    }
    answer
}

/// A request for the node's own router, as the paired local client acting
/// for `device`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn loopback_request(
    node: &Node,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
    device: Option<&str>,
    via: &str,
    main_port: u16,
) -> Vec<u8> {
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{main_port}\r\nX-Ob-Pair: {}\r\n{REMOTE_VIA_HEADER}: {via}\r\n", node.pairing_token());
    if let Some(device) = device {
        raw.push_str(&format!("{REMOTE_DEVICE_HEADER}: {device}\r\n"));
    }
    if let Some(content_type) = content_type {
        raw.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    raw.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut raw = raw.into_bytes();
    raw.extend_from_slice(body);
    raw
}

pub(crate) fn unauthorized(code: &str, message: &str) -> Vec<u8> {
    json_body(
        "401 Unauthorized",
        &json!({ "error": message, "code": code }),
        None,
    )
}

fn misdirected() -> Vec<u8> {
    http_response_cors(
        "421 Misdirected Request",
        "text/plain",
        b"This is not the address of this node.",
        None,
    )
}
