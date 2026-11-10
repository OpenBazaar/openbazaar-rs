//! The life of a pairing code on the node's console, so a pairing that fails
//! can be read from the logs of both sides: the code made, its onion
//! publishing, published and reached, a request arriving and where, a
//! refusal and why, the owner's decision, the code expiring, its onion closing.
//!
//! A line names a code by its id and an onion by a shortened address. No line
//! carries a code's words, secret or link, or a key.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::pairing::Code;
use super::protocol::pairing_onion;

const WINDOW: Duration = Duration::from_secs(60);
const BURST: u32 = 20;

/// At most [`BURST`] lines in [`WINDOW`]. The rest are counted.
pub(crate) struct Limit {
    since: Option<Instant>,
    shown: u32,
    dropped: u32,
}

impl Limit {
    pub(crate) const fn new() -> Self {
        Self {
            since: None,
            shown: 0,
            dropped: 0,
        }
    }

    /// `None` to drop this line; otherwise how many were dropped before it.
    pub(crate) fn allow(&mut self, now: Instant) -> Option<u32> {
        if self.since.is_none_or(|since| now.duration_since(since) >= WINDOW) {
            self.since = Some(now);
            self.shown = 0;
        }
        if self.shown >= BURST {
            self.dropped += 1;
            return None;
        }
        self.shown += 1;
        Some(std::mem::take(&mut self.dropped))
    }
}

/// `abcdef…wxyz` for the 56 characters of an onion name: enough to match two
/// logs, too little to dial.
fn short(name: &str) -> String {
    format!("{}…{}", &name[..6], &name[name.len() - 4..])
}

/// `text` on one line of at most 300 characters, with every whole onion name
/// in it shortened. For reasons that come from Arti.
pub(crate) fn scrub(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    for ch in text.chars().chain([' ']) {
        if matches!(ch, 'a'..='z' | '2'..='7') {
            run.push(ch);
            continue;
        }
        out.push_str(&if run.len() == 56 { short(&run) } else { std::mem::take(&mut run) });
        run.clear();
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    out.trim_end().chars().take(300).collect()
}

/// `code 3fa9c1b2d4e6 (onion abcdef…wxyz.onion)`
pub(crate) fn label(code: &Code) -> String {
    format!("code {} (onion {})", code.id, scrub(&pairing_onion(&code.secret)))
}

pub(crate) fn line(text: String) {
    #[cfg(test)]
    tests::LINES.with_borrow_mut(|lines| lines.push(text.clone()));
    eprintln!("ob-node: pairing: {text}");
}

/// A pairing request the node did not accept. Anyone who finds a pairing
/// onion can send these, so they are rate-limited.
pub(crate) fn refused(arrived: &str, answer: &[u8]) {
    static LIMIT: Mutex<Limit> = Mutex::new(Limit::new());
    let allowed = LIMIT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .allow(Instant::now());
    let Some(dropped) = allowed else {
        return;
    };
    let more = if dropped > 0 {
        format!(" ({dropped} more refusals not shown)")
    } else {
        String::new()
    };
    line(format!("{}{more}", refusal(arrived, answer)));
}

/// The status and the `error` text of the node's own answer.
fn refusal(arrived: &str, answer: &[u8]) -> String {
    let text = String::from_utf8_lossy(answer);
    let status = text.lines().next().unwrap_or_default().trim_start_matches("HTTP/1.1 ");
    let reason = text
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(body).ok())
        .and_then(|body| body["error"].as_str().map(String::from))
        .unwrap_or_default();
    format!("a request by {arrived} was refused: {status}: {reason}")
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        /// Every pairing line this thread logged.
        pub(crate) static LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn onion_names_are_shortened_and_reasons_stay_on_one_line() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        assert_eq!(
            scrub(&format!("could not reach {onion}.onion:\n  timed out")),
            "could not reach 25njqa…5sid.onion:   timed out"
        );
        assert_eq!(scrub(onion), "25njqa…5sid");
        assert_eq!(scrub("tor: Onion Service not found"), "tor: Onion Service not found");
        assert_eq!(scrub(&"é".repeat(400)).chars().count(), 300);
    }

    #[test]
    fn a_refusal_is_logged_with_the_answer_the_device_got() {
        let answer = crate::http::json_error("401 Unauthorized", "That pairing code is wrong, used, or expired.", None);
        assert_eq!(
            refusal("a pairing onion", &answer),
            "a request by a pairing onion was refused: 401 Unauthorized: That pairing code is wrong, used, or expired."
        );
    }

    #[test]
    fn a_burst_of_lines_is_cut_and_counted() {
        let start = Instant::now();
        let mut limit = Limit::new();
        let shown = (0..50).filter(|_| limit.allow(start).is_some()).count();
        assert_eq!(shown, BURST as usize);
        assert_eq!(limit.allow(start + WINDOW), Some(30), "the next window says what was dropped");
        assert_eq!(limit.allow(start + WINDOW), Some(0));
    }
}
