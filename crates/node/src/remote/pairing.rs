//! Pairing codes and pairing requests (protocol v2, "Pairing").
//!
//! A code is an 80-bit secret shown on the node's console, its own app, the
//! LAN setup page, or an already paired device, as 8 words or an
//! `openbazaar://pair` link. It lives [`CODE_LIFETIME`], at most
//! [`MAX_LIVE`] are open, and the first request that proves the secret takes
//! it. That request waits until the owner compares its emoji and approves or
//! rejects it on the node; either way the code is spent. A later attempt from
//! the same device name or the same device key replaces that pending request.
//! Rejected and expired attempts leave the pending list. Bad proofs are
//! counted node-wide, and past [`MAX_FAILURES`] in [`FAILURE_WINDOW`]
//! pairing pauses.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bitcoin::hashes::{sha256, Hash};
use serde_json::Value;

use super::auth::decode_hex;
use super::pair_log;
use super::pair_ready::{self, Onion};
use super::protocol::{base32, decode_base32, parse_words, PairRequest, SECRET_LEN};
use crate::vault_key::{encode_hex, random_bytes};

pub(crate) const CODE_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// A decided request answers `/pair/status` this long, then reads as expired.
const DECIDED_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_LIVE: usize = 4;
const MAX_FAILURES: usize = 20;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
pub(crate) const LINK_PREFIX: &str = "openbazaar://pair?";

#[derive(Clone)]
pub(crate) struct Code {
    pub secret: [u8; SECRET_LEN],
    /// Names this code's pairing onion service.
    pub id: String,
    /// When the code's [`CODE_LIFETIME`] began: when it was made, or when the
    /// page stopped holding its QR for the onion.
    created: Instant,
    /// When the code was made.
    issued: Instant,
    by: Option<String>,
    pub taken: bool,
    /// Turns true once the node's own Tor client has reached this code's onion.
    reached: Option<Arc<AtomicBool>>,
    /// The owner's page is still holding the QR for the onion.
    held: bool,
}

impl Code {
    pub(crate) fn expires_in(&self, now: Instant) -> Duration {
        CODE_LIFETIME.saturating_sub(now.duration_since(self.created))
    }

    /// How this code's onion is doing ([`pair_ready`]), or `None` for a code
    /// that has no onion because Tor is off.
    pub(crate) fn onion(&self, now: Instant) -> Option<Onion> {
        let reached = self.reached.as_ref()?.load(Ordering::Relaxed);
        Some(pair_ready::state(reached, now.duration_since(self.issued)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    Pending,
    Approved(Value),
    Rejected,
}

#[derive(Clone)]
pub(crate) struct Request {
    pub id: String,
    pub fields: PairRequest,
    pub secret: [u8; SECRET_LEN],
    pub by: Option<String>,
    pub state: State,
    pub code: String,
    created: Instant,
    decided: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Wrong,
    Taken,
    Paused,
}

/// A repeat attempt is the same phone when the device key matches or the
/// name on the setup page matches. A retry can mint a new key and keep the name.
fn same_device(left: &PairRequest, right: &PairRequest) -> bool {
    left.key == right.key || left.name == right.name
}

#[derive(Default)]
pub(crate) struct Pairing {
    codes: Vec<Code>,
    requests: Vec<Request>,
    failures: VecDeque<Instant>,
}

impl Pairing {
    fn prune(&mut self, now: Instant) {
        // The wait for a code's onion does not come out of its lifetime. A
        // code nobody looked at since was held no longer than the limit.
        for code in self.codes.iter_mut().filter(|code| code.held) {
            let onion = code.onion(now);
            if onion != Some(Onion::Publishing) {
                code.held = false;
                code.created = now.min(code.issued + pair_ready::SLOW_AFTER);
            }
            if onion == Some(Onion::Slow) {
                pair_log::line(format!(
                    "code {}: its onion did not answer in {}s; the page now shows the code with a warning",
                    code.id,
                    pair_ready::SLOW_AFTER.as_secs()
                ));
            }
        }
        let requests = &self.requests;
        self.codes.retain(|code| {
            let live = now.duration_since(code.created) < CODE_LIFETIME;
            if !live {
                let how = if code.taken { "with a request still waiting" } else { "unused" };
                pair_log::line(format!("code {} expired {how}", code.id));
            }
            live
                && (!code.taken
                    || requests
                        .iter()
                        .any(|request| request.code == code.id && request.state == State::Pending))
        });
        let codes = &self.codes;
        self.requests.retain(|request| match request.decided {
            Some(at) => now.duration_since(at) < DECIDED_LIFETIME,
            None => codes.iter().any(|code| code.id == request.code),
        });
        while self
            .failures
            .front()
            .is_some_and(|at| now.duration_since(*at) >= FAILURE_WINDOW)
        {
            self.failures.pop_front();
        }
    }

    /// A new code. The oldest open code closes when [`MAX_LIVE`] are open.
    pub(crate) fn issue(&mut self, by: Option<String>, now: Instant) -> Code {
        self.prune(now);
        if self.codes.len() >= MAX_LIVE {
            let oldest = self.codes.remove(0);
            pair_log::line(format!("code {} closed to make room for a new one", oldest.id));
            self.requests.retain(|request| request.code != oldest.id);
        }
        let secret = random_bytes::<SECRET_LEN>();
        let id = encode_hex(&sha256::Hash::hash(&secret).to_byte_array()[..6]);
        let code = Code {
            secret,
            id,
            created: now,
            issued: now,
            by,
            taken: false,
            reached: None,
            held: false,
        };
        self.codes.push(code.clone());
        code
    }

    /// Hold `id`'s QR on the owner's page until `reached` turns true or
    /// [`pair_ready::SLOW_AFTER`] passes. Returns the code as it now is.
    pub(crate) fn watch_onion(&mut self, id: &str, reached: Arc<AtomicBool>) -> Option<Code> {
        let code = self.codes.iter_mut().find(|code| code.id == id)?;
        code.reached = Some(reached);
        code.held = true;
        Some(code.clone())
    }

    pub(crate) fn live(&mut self, now: Instant) -> Vec<Code> {
        self.prune(now);
        self.codes.clone()
    }

    /// Match a request to the code its proof was made with.
    ///
    /// One device has one pending attempt. A repeat from the same device name
    /// or the same device key replaces the previous one. A code a different
    /// device already took stays taken.
    pub(crate) fn request(
        &mut self,
        fields: PairRequest,
        proof: &str,
        now: Instant,
    ) -> Result<Request, Refusal> {
        self.prune(now);
        if self.failures.len() >= MAX_FAILURES {
            return Err(Refusal::Paused);
        }
        let Some(index) = self
            .codes
            .iter()
            .position(|code| fields.proves(&code.secret, proof))
        else {
            self.failures.push_back(now);
            return Err(Refusal::Wrong);
        };
        let code_id = self.codes[index].id.clone();
        if self.codes[index].taken && !self.held_by_same_device(&code_id, &fields) {
            return Err(Refusal::Taken);
        }
        self.supersede(&fields, &code_id);
        let code = self
            .codes
            .iter_mut()
            .find(|code| code.id == code_id)
            .expect("the code this attempt proved");
        code.taken = true;
        let request = Request {
            id: encode_hex(&random_bytes::<16>()),
            fields,
            secret: code.secret,
            by: code.by.clone(),
            state: State::Pending,
            code: code_id,
            created: now,
            decided: None,
        };
        self.requests.push(request.clone());
        Ok(request)
    }

    /// True when this code's pending attempt is the device asking again.
    fn held_by_same_device(&self, code_id: &str, fields: &PairRequest) -> bool {
        self.requests.iter().any(|request| {
            request.code == code_id
                && request.state == State::Pending
                && same_device(&request.fields, fields)
        })
    }

    /// Drop this device's pending attempts. `keep` is the code the new
    /// attempt proved, so that code stays open and the older ones close.
    fn supersede(&mut self, fields: &PairRequest, keep: &str) {
        let drop_codes: Vec<String> = self
            .requests
            .iter()
            .filter(|request| {
                request.code != keep
                    && request.state == State::Pending
                    && same_device(&request.fields, fields)
            })
            .map(|request| request.code.clone())
            .collect();
        self.requests.retain(|request| {
            !(request.state == State::Pending && same_device(&request.fields, fields))
        });
        self.codes
            .retain(|code| code.id == keep || !drop_codes.iter().any(|id| id == &code.id));
    }

    pub(crate) fn pending(&mut self, now: Instant) -> Vec<(Request, Duration)> {
        self.prune(now);
        self.requests
            .iter()
            .filter(|request| request.state == State::Pending)
            .map(|request| (request.clone(), now.duration_since(request.created)))
            .collect()
    }

    pub(crate) fn get(&mut self, id: &str, now: Instant) -> Option<Request> {
        self.prune(now);
        self.requests
            .iter()
            .find(|request| request.id == id)
            .cloned()
    }

    /// Record the owner's decision; the code is spent either way.
    pub(crate) fn decide(&mut self, id: &str, state: State, now: Instant) {
        if let Some(request) = self
            .requests
            .iter_mut()
            .find(|request| request.id == id && request.state == State::Pending)
        {
            request.state = state;
            request.decided = Some(now);
            let code = request.code.clone();
            self.codes.retain(|live| live.id != code);
        }
    }

    /// Close every code a removed device made, and its pending requests.
    pub(crate) fn drop_from(&mut self, devices: &[String]) {
        let made_by = |by: &Option<String>| by.as_ref().is_some_and(|by| devices.contains(by));
        self.codes.retain(|code| !made_by(&code.by));
        self.requests
            .retain(|request| !(request.state == State::Pending && made_by(&request.by)));
    }
}

/// Where a typed or scanned code points.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Link {
    pub secret: [u8; SECRET_LEN],
    pub node_key: Option<[u8; 32]>,
    pub onion: Option<String>,
    pub lan: Vec<String>,
}

pub(crate) fn link(
    secret: &[u8; SECRET_LEN],
    node_public: &[u8; 32],
    onion: Option<&str>,
    lan: &[String],
) -> String {
    let mut text = format!(
        "{LINK_PREFIX}v=2&s={}&k={}",
        base32(secret),
        encode_hex(node_public)
    );
    if let Some(onion) = onion {
        text.push_str(&format!("&o={}", onion.trim_end_matches(".onion")));
    }
    for address in lan {
        text.push_str(&format!("&l={address}"));
    }
    text
}

/// An `openbazaar://pair` link, or 8 words.
pub(crate) fn parse(text: &str) -> Option<Link> {
    let text = text.trim();
    let Some(query) = text.strip_prefix(LINK_PREFIX) else {
        return parse_words(text).map(|secret| Link {
            secret,
            ..Link::default()
        });
    };
    let mut link = Link::default();
    let mut secret = None;
    for (name, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        match name {
            "v" if value != "2" => return None,
            "s" => secret = decode_base32(value).and_then(|bytes| bytes.try_into().ok()),
            "k" => link.node_key = decode_hex(value).and_then(|bytes| bytes.try_into().ok()),
            "o" if value.len() == 56 && value.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                link.onion = Some(format!("{}.onion", value.to_lowercase()))
            }
            "l" if !value.is_empty() && value.len() <= 64 => link.lan.push(value.to_string()),
            _ => {}
        }
    }
    link.secret = secret?;
    Some(link)
}
