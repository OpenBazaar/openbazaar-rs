//! Content-free wake-ups for devices that turned push on.
//!
//! The node never talks to Apple or Google. With `--push-gateway
//! http://….onion` it asks that gateway, over Tor, to wake a device token:
//! `POST /v1/wake {platform, token}`. The gateway learns that some token
//! should wake up, not which node asked, what happened, or where the node is.
//! Apple or Google learn that the app got a silent ping. A wake-up goes out
//! when the orders or the wallet change (a new order, a message, a payment
//! seen), 0 to 90 s later and at most once per device per 10 minutes, so its
//! timing says little about the event.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tormail::OnionAddress;

use super::devices::Push;
use super::sync;
use crate::Node;

const MIN_GAP: Duration = Duration::from_secs(10 * 60);
const MAX_JITTER_SECS: u64 = 90;
const SEND_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_TOKEN: usize = 512;

#[derive(Default)]
pub(crate) struct PushState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    seen: Option<String>,
    due: HashMap<String, Instant>,
    sent: HashMap<String, Instant>,
}

pub(crate) struct Gateway {
    pub onion: OnionAddress,
    pub prefix: String,
}

/// `http://{56 letters}.onion[/prefix]`; nothing else, so a wake-up never
/// leaves Tor.
pub(crate) fn gateway_target(url: &str) -> Result<Gateway, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("--push-gateway must be an http://….onion address")?;
    let (host, prefix) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, prefix)| (host, prefix));
    let onion = OnionAddress::parse(host)
        .map_err(|_| "--push-gateway must be a v3 .onion address".to_string())?;
    let prefix = prefix.trim_end_matches('/');
    Ok(Gateway {
        onion,
        prefix: if prefix.is_empty() {
            String::new()
        } else {
            format!("/{prefix}")
        },
    })
}

pub(crate) fn parse_token(value: &Value) -> Option<Push> {
    let platform = value["platform"]
        .as_str()
        .filter(|platform| ["apns", "fcm"].contains(platform))?;
    let token = value["token"].as_str().filter(|token| {
        (1..=MAX_TOKEN).contains(&token.len())
            && token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b":_-.".contains(&byte))
    })?;
    Some(Push {
        platform: platform.into(),
        token: token.into(),
    })
}

/// Called every few seconds: notice changes, send the wake-ups that are due.
pub(crate) fn check(node: &Node, main_port: u16) {
    let remote = &node.remote;
    let Some(gateway) = remote
        .config()
        .push_gateway
        .as_deref()
        .and_then(|url| gateway_target(url).ok())
    else {
        return;
    };
    let targets: Vec<(String, Push)> = remote
        .devices()
        .list()
        .iter()
        .filter_map(|device| Some((device.id.clone(), device.push.clone()?)))
        .collect();
    if targets.is_empty() {
        return;
    }
    let fingerprint = {
        let _one = super::dispatch_lock();
        let (_, orders) = sync::run(node, main_port, "GET", "/orders", b"", None, "local");
        let (_, wallet) = sync::run(node, main_port, "GET", "/wallet", b"", None, "local");
        sync::etag(&json!([orders, wallet]))
    };
    let now = Instant::now();
    let due: Vec<(String, Push)> = {
        let mut inner = remote
            .push
            .inner
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let changed = inner
            .seen
            .replace(fingerprint.clone())
            .is_some_and(|seen| seen != fingerprint);
        if changed {
            for (id, _) in &targets {
                let earliest = inner.sent.get(id).map_or(now, |sent| *sent + MIN_GAP);
                let at = earliest
                    .max(now + Duration::from_secs(rand::random::<u64>() % (MAX_JITTER_SECS + 1)));
                inner.due.entry(id.clone()).or_insert(at);
            }
        }
        let ready: Vec<String> = inner
            .due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ready {
            inner.due.remove(id);
            inner.sent.insert(id.clone(), now);
        }
        targets
            .into_iter()
            .filter(|(id, _)| ready.contains(id))
            .collect()
    };
    let Some(tor) = remote.running_tor() else {
        return;
    };
    for (id, push) in due {
        let body = json!({ "platform": push.platform, "token": push.token }).to_string();
        let request = format!(
            "POST {}/v1/wake HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            gateway.prefix,
            gateway.onion,
            body.len()
        );
        match tor.exchange_within(&gateway.onion, request.as_bytes(), 4096, SEND_TIMEOUT) {
            Ok(answer) if sync::split_response(&answer).0 < 300 => {}
            Ok(answer) => eprintln!(
                "ob-node: push gateway refused a wake-up for {id}: {}",
                sync::split_response(&answer).0
            ),
            Err(err) => eprintln!("ob-node: push gateway: {err}"),
        }
    }
}
