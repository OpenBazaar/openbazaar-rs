//! `POST /pair`: trade a one-time code from the node's terminal for the token.
//!
//! No request gets the pairing token for free. The desktop app reads
//! `pairing.token` itself and hands it to its own webview, and starts the
//! node with `--no-pair-codes`. A shop in a browser posts `{}` to ask for a
//! code; the node prints the code on its terminal (never in a response), and
//! the user types it into the shop, which posts `{"code": "..."}` and gets the
//! token back. A web page or another user's process can make the node print
//! a code but cannot read it.
//!
//! A code is 8 characters from a 30-symbol alphabet (about 39 bits), lives
//! [`CODE_LIFETIME`], and dies after [`MAX_ATTEMPTS`] wrong guesses. A new
//! code is issued at most every [`ISSUE_INTERVAL`], so guessing runs at
//! under one try per second against a 2^39 space.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

use crate::http_guard::secret_eq;
use crate::http::{json_body, json_error};
use crate::Node;

const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTVWXYZ23456789";
const CODE_LEN: usize = 8;
const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);
const MAX_ATTEMPTS: u32 = 5;
const ISSUE_INTERVAL: Duration = Duration::from_secs(10);
const MAX_PAIR_BODY: usize = 256;

struct Live {
    code: String,
    issued: Instant,
    attempts: u32,
}

pub(crate) struct PairingCodes {
    enabled: bool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    live: Option<Live>,
    last_issue: Option<Instant>,
}

/// What a `POST /pair` did, before it becomes a response.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Disabled,
    /// A code was printed, or one is already waiting.
    CodeNeeded,
    TooSoon,
    Wrong,
    Paired,
}

impl PairingCodes {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::new(State::default()),
        }
    }

    /// `announce` receives each new code; the node prints it on stderr.
    pub(crate) fn attempt(&self, presented: Option<&str>, now: Instant, announce: impl FnOnce(&str)) -> Outcome {
        if !self.enabled {
            return Outcome::Disabled;
        }
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state
            .live
            .as_ref()
            .is_some_and(|live| now.duration_since(live.issued) >= CODE_LIFETIME || live.attempts >= MAX_ATTEMPTS)
        {
            state.live = None;
        }
        let Some(presented) = presented else {
            if state.live.is_some() {
                return Outcome::CodeNeeded;
            }
            if state.last_issue.is_some_and(|last| now.duration_since(last) < ISSUE_INTERVAL) {
                return Outcome::TooSoon;
            }
            let code = new_code();
            announce(&display(&code));
            state.live = Some(Live {
                code,
                issued: now,
                attempts: 0,
            });
            state.last_issue = Some(now);
            return Outcome::CodeNeeded;
        };
        let Some(live) = state.live.as_mut() else {
            return Outcome::Wrong;
        };
        if secret_eq(Some(&normalize(presented)), &live.code) {
            state.live = None;
            return Outcome::Paired;
        }
        live.attempts += 1;
        Outcome::Wrong
    }
}

fn new_code() -> String {
    let mut rng = StdRng::from_os_rng();
    (0..CODE_LEN)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect()
}

fn display(code: &str) -> String {
    format!("{}-{}", &code[..CODE_LEN / 2], &code[CODE_LEN / 2..])
}

/// Upper case, without the dash or spaces a user may type.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|ch| !matches!(ch, '-' | ' '))
        .map(|ch| ch.to_ascii_uppercase())
        .collect()
}

pub(crate) fn response(node: &Node, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if body.len() > MAX_PAIR_BODY {
        return json_error("400 Bad Request", "That request is too large.", origin);
    }
    let parsed: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => return json_error("400 Bad Request", "The request could not be read.", origin),
        }
    };
    let code = parsed.get("code").and_then(Value::as_str);
    let outcome = node.pairing_codes.attempt(code, Instant::now(), |code| {
        eprintln!("ob-node: pairing code {code} (valid 5 minutes, one use). Type it into the shop to pair it.");
    });
    match outcome {
        Outcome::Paired => json_body("200 OK", &json!({ "token": node.pairing_token() }), origin),
        Outcome::CodeNeeded => json_body(
            "401 Unauthorized",
            &json!({
                "error": "Type the pairing code shown in your node's terminal.",
                "codeRequired": true,
            }),
            origin,
        ),
        Outcome::Wrong => json_error("401 Unauthorized", "That pairing code is wrong or has expired.", origin),
        Outcome::TooSoon => json_error("429 Too Many Requests", "Wait a few seconds before asking for a new code.", origin),
        Outcome::Disabled => json_error(
            "403 Forbidden",
            "Pairing codes are off on this node. The desktop app pairs its own window.",
            origin,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(codes: &PairingCodes, now: Instant) -> Option<String> {
        let mut printed = None;
        let outcome = codes.attempt(None, now, |code| printed = Some(code.to_string()));
        assert_eq!(outcome, Outcome::CodeNeeded);
        printed
    }

    #[test]
    fn a_printed_code_pairs_once() {
        let codes = PairingCodes::new(true);
        let now = Instant::now();
        let code = issue(&codes, now).unwrap();
        assert_eq!(code.len(), CODE_LEN + 1);
        assert_eq!(issue(&codes, now), None, "a waiting code is not printed again");
        assert_eq!(codes.attempt(Some(&code.to_lowercase()), now, |_| ()), Outcome::Paired);
        assert_eq!(codes.attempt(Some(&code), now, |_| ()), Outcome::Wrong, "one use");
    }

    #[test]
    fn wrong_guesses_and_time_kill_a_code() {
        let codes = PairingCodes::new(true);
        let now = Instant::now();
        let code = issue(&codes, now).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(codes.attempt(Some("AAAA-AAAA"), now, |_| ()), Outcome::Wrong);
        }
        assert_eq!(codes.attempt(Some(&code), now, |_| ()), Outcome::Wrong);
        assert_eq!(codes.attempt(None, now, |_| ()), Outcome::TooSoon);

        let later = now + ISSUE_INTERVAL;
        let code = issue(&codes, later).unwrap();
        assert_eq!(codes.attempt(Some(&code), later + CODE_LIFETIME, |_| ()), Outcome::Wrong);
    }

    #[test]
    fn disabled_codes_never_print() {
        let codes = PairingCodes::new(false);
        assert_eq!(codes.attempt(None, Instant::now(), |_| panic!("printed")), Outcome::Disabled);
    }
}
