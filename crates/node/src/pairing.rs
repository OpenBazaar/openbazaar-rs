//! `POST /pair`: trade a one-time code for the token, or show that code to a
//! shop that is already paired.
//!
//! No unpaired request gets the pairing token or the live code. The desktop
//! app reads `pairing.token` itself and hands it to its own webview, and starts
//! the node with `--no-pair-codes`. A shop in a browser posts `{}` to ask for a
//! code; the node prints the code on its terminal (never in that unpaired
//! response), and the user types it into the shop, which posts `{"code": "..."}`
//! and gets the token back. A paired shop posting `{}` gets the same `XXXX-XXXX`
//! in JSON so Settings can show it. A web page or another user's process can
//! make the node print a code but cannot read it.
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

use crate::http::{json_body, json_error, Client};
use crate::http_guard::secret_eq;
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
    pub(crate) fn attempt(
        &self,
        presented: Option<&str>,
        now: Instant,
        announce: impl FnOnce(&str),
    ) -> Outcome {
        if !self.enabled {
            return Outcome::Disabled;
        }
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state.live.as_ref().is_some_and(|live| {
            now.duration_since(live.issued) >= CODE_LIFETIME || live.attempts >= MAX_ATTEMPTS
        }) {
            state.live = None;
        }
        let Some(presented) = presented else {
            if state.live.is_some() {
                return Outcome::CodeNeeded;
            }
            if state
                .last_issue
                .is_some_and(|last| now.duration_since(last) < ISSUE_INTERVAL)
            {
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

    /// Issue a code a paired shop can show. A live code is returned as-is.
    pub(crate) fn reveal(
        &self,
        now: Instant,
        announce: impl FnOnce(&str),
    ) -> Result<String, Outcome> {
        if !self.enabled {
            return Err(Outcome::Disabled);
        }
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state.live.as_ref().is_some_and(|live| {
            now.duration_since(live.issued) >= CODE_LIFETIME || live.attempts >= MAX_ATTEMPTS
        }) {
            state.live = None;
        }
        if let Some(live) = &state.live {
            return Ok(display(&live.code));
        }
        if state
            .last_issue
            .is_some_and(|last| now.duration_since(last) < ISSUE_INTERVAL)
        {
            return Err(Outcome::TooSoon);
        }
        let code = new_code();
        let shown = display(&code);
        announce(&shown);
        state.live = Some(Live {
            code,
            issued: now,
            attempts: 0,
        });
        state.last_issue = Some(now);
        Ok(shown)
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

pub(crate) fn response(
    node: &Node,
    client: &Client,
    head: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if body.len() > MAX_PAIR_BODY {
        return json_error("400 Bad Request", "That request is too large.", origin);
    }
    let parsed: Value = if body.iter().all(u8::is_ascii_whitespace) {
        json!({})
    } else {
        match serde_json::from_slice(body) {
            Ok(value) => value,
            Err(_) => {
                return json_error("400 Bad Request", "The request could not be read.", origin)
            }
        }
    };
    let presented = parsed.get("code").and_then(Value::as_str);
    if presented.is_none() && crate::browser::allows(node, client, head) {
        return reveal_to_shop(node, origin);
    }
    trade(node, presented, origin)
}

fn announce_code(code: &str) {
    eprintln!(
        "ob-node: pairing code {code} (valid 5 minutes, one use). Type it into the shop to pair it."
    );
}

fn reveal_to_shop(node: &Node, origin: Option<&str>) -> Vec<u8> {
    match node.pairing_codes.reveal(Instant::now(), announce_code) {
        Ok(code) => {
            let body = json!({ "code": code });
            debug_assert!(
                !body.to_string().contains(node.pairing_token()),
                "Settings mint must not include the pairing token"
            );
            json_body("200 OK", &body, origin)
        }
        Err(Outcome::TooSoon) => json_error(
            "429 Too Many Requests",
            "Wait a few seconds before asking for a new code.",
            origin,
        ),
        Err(Outcome::Disabled) => json_error(
            "403 Forbidden",
            "Pairing codes are off on this node. The desktop app pairs its own window.",
            origin,
        ),
        Err(_) => json_error(
            "401 Unauthorized",
            "Type the pairing code from your node.",
            origin,
        ),
    }
}

fn trade(node: &Node, presented: Option<&str>, origin: Option<&str>) -> Vec<u8> {
    let outcome = node
        .pairing_codes
        .attempt(presented, Instant::now(), announce_code);
    match outcome {
        Outcome::Paired => json_body("200 OK", &json!({ "token": node.pairing_token() }), origin),
        Outcome::CodeNeeded => json_body(
            "401 Unauthorized",
            &json!({
                "error": "Type the pairing code from your node.",
                "codeRequired": true,
            }),
            origin,
        ),
        Outcome::Wrong => json_error(
            "401 Unauthorized",
            "That pairing code is wrong or has expired.",
            origin,
        ),
        Outcome::TooSoon => json_error(
            "429 Too Many Requests",
            "Wait a few seconds before asking for a new code.",
            origin,
        ),
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
        assert_eq!(
            issue(&codes, now),
            None,
            "a waiting code is not printed again"
        );
        assert_eq!(
            codes.attempt(Some(&code.to_lowercase()), now, |_| ()),
            Outcome::Paired
        );
        assert_eq!(
            codes.attempt(Some(&code), now, |_| ()),
            Outcome::Wrong,
            "one use"
        );
    }

    #[test]
    fn wrong_guesses_and_time_kill_a_code() {
        let codes = PairingCodes::new(true);
        let now = Instant::now();
        let code = issue(&codes, now).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(
                codes.attempt(Some("AAAA-AAAA"), now, |_| ()),
                Outcome::Wrong
            );
        }
        assert_eq!(codes.attempt(Some(&code), now, |_| ()), Outcome::Wrong);
        assert_eq!(codes.attempt(None, now, |_| ()), Outcome::TooSoon);

        let later = now + ISSUE_INTERVAL;
        let code = issue(&codes, later).unwrap();
        assert_eq!(
            codes.attempt(Some(&code), later + CODE_LIFETIME, |_| ()),
            Outcome::Wrong
        );
    }

    #[test]
    fn disabled_codes_never_print() {
        let codes = PairingCodes::new(false);
        assert_eq!(
            codes.attempt(None, Instant::now(), |_| panic!("printed")),
            Outcome::Disabled
        );
        assert_eq!(
            codes.reveal(Instant::now(), |_| panic!("printed")),
            Err(Outcome::Disabled)
        );
    }

    #[test]
    fn a_paired_shop_can_read_the_code_the_redeem_path_accepts() {
        let codes = PairingCodes::new(true);
        let now = Instant::now();
        let mut printed = None;
        let shown = codes
            .reveal(now, |code| printed = Some(code.to_string()))
            .unwrap();
        assert_eq!(printed.as_deref(), Some(shown.as_str()));
        assert_eq!(shown.len(), CODE_LEN + 1);
        assert_eq!(
            codes.reveal(now, |_| panic!("printed again")).unwrap(),
            shown
        );
        assert_eq!(codes.attempt(Some(&shown), now, |_| ()), Outcome::Paired);
        assert_eq!(
            codes.attempt(Some(&shown), now, |_| ()),
            Outcome::Wrong,
            "one use"
        );
    }

    #[test]
    fn a_revealed_code_expires_like_a_printed_one() {
        let codes = PairingCodes::new(true);
        let now = Instant::now();
        let shown = codes.reveal(now, |_| ()).unwrap();
        assert_eq!(
            codes.attempt(Some(&shown), now + CODE_LIFETIME, |_| ()),
            Outcome::Wrong
        );
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::harness::TempDir;
    use crate::http::{respond, Client};
    use crate::{load_catalog, NetworkMode, Node};

    fn node() -> (TempDir, Node) {
        let dir = TempDir::new();
        let catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let node = Node::open(&dir.0, catalog).unwrap();
        (dir, node)
    }

    fn call(node: &Node, raw: &str, loopback: bool) -> String {
        let response = respond(
            node,
            raw.as_bytes(),
            Client {
                loopback,
                port: 8787,
            },
        );
        String::from_utf8(response).unwrap()
    }

    fn json_of(response: &str) -> Value {
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap_or(Value::Null)
    }

    #[test]
    fn settings_mint_returns_a_code_an_unpaired_browser_can_redeem() {
        let (_dir, node) = node();
        let token = node.pairing_token().to_string();
        let asked = call(
            &node,
            "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: 2\r\n\r\n{}",
            true,
        );
        assert!(asked.contains("401"), "{asked}");
        let asked_body = json_of(&asked);
        assert!(asked_body.get("code").is_none(), "{asked}");
        assert_ne!(asked_body.get("token"), Some(&Value::String(token.clone())));
        assert!(!asked.contains(&token), "{asked}");

        let minted = call(
            &node,
            &format!(
                "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nX-Ob-Pair: {token}\r\nContent-Length: 2\r\n\r\n{{}}"
            ),
            true,
        );
        assert!(minted.starts_with("HTTP/1.1 200"), "{minted}");
        assert!(!minted.contains(&token), "{minted}");
        let code = json_of(&minted)["code"].as_str().expect("code").to_string();
        assert_eq!(code.len(), CODE_LEN + 1);
        assert!(!code.contains(&token));

        let request = format!(r#"{{"code":"{code}"}}"#);
        let traded = call(
            &node,
            &format!(
                "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: {}\r\n\r\n{request}",
                request.len()
            ),
            false,
        );
        assert!(traded.contains("200"), "{traded}");
        assert_eq!(json_of(&traded)["token"], token);
        let again = call(
            &node,
            &format!(
                "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: {}\r\n\r\n{request}",
                request.len()
            ),
            true,
        );
        assert!(again.contains("401"), "one use: {again}");
    }

    #[test]
    fn a_non_loopback_request_cannot_read_the_code_without_pairing() {
        let (_dir, node) = node();
        let token = node.pairing_token().to_string();
        let remote = call(
            &node,
            "POST /pair HTTP/1.1\r\nHost: 127.0.0.1:8787\r\nContent-Length: 2\r\n\r\n{}",
            false,
        );
        assert!(remote.contains("401"), "{remote}");
        assert!(json_of(&remote).get("code").is_none(), "{remote}");
        assert!(!remote.contains(&token), "{remote}");
    }
}
