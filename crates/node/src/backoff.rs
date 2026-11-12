//! When to ask a server this project does not run again, after it failed or refused.
//!
//! Each failure doubles the wait, from `first` up to `cap`. A server that
//! names its own wait (`Retry-After`) or refuses outright (403, 429) is left
//! alone at least that long, and up to an hour while it goes on refusing.
//! One answer clears the count.

use std::time::{Duration, Instant};

/// The least wait after a server refuses (403, 429) without naming one.
const REFUSED: Duration = Duration::from_secs(15 * 60);
/// A server that goes on refusing is still asked this often.
const REFUSED_CAP: Duration = Duration::from_secs(60 * 60);
/// A `Retry-After` longer than this is not believed.
const LONGEST_ASKED: Duration = Duration::from_secs(24 * 60 * 60);

/// A failed fetch is tried again after half a minute, then ever less often,
/// up to five minutes. Price servers and public Esplora share this.
pub(crate) const fn for_prices() -> Backoff {
    Backoff::new(Duration::from_secs(30), Duration::from_secs(5 * 60))
}

pub(crate) struct Backoff {
    first: Duration,
    cap: Duration,
    failures: u32,
    until: Option<Instant>,
}

impl Backoff {
    pub(crate) const fn new(first: Duration, cap: Duration) -> Self {
        Self {
            first,
            cap,
            failures: 0,
            until: None,
        }
    }

    /// `true` when the server may be asked at `now`.
    pub(crate) fn ready(&self, now: Instant) -> bool {
        self.until.is_none_or(|until| now >= until)
    }

    pub(crate) fn ok(&mut self) {
        self.failures = 0;
        self.until = None;
    }

    /// Run `ask` unless the server is being left alone, and count how it
    /// went. The error of `ask` is the wait the server asked for, if any.
    pub(crate) fn ask<T>(
        &mut self,
        ask: impl FnOnce() -> Result<T, Option<Duration>>,
    ) -> Option<T> {
        if !self.ready(Instant::now()) {
            return None;
        }
        match ask() {
            Ok(answer) => {
                self.ok();
                Some(answer)
            }
            Err(asked) => {
                self.fail(Instant::now(), asked);
                None
            }
        }
    }

    /// Count a failure at `now`. `asked` is a wait the server itself named
    /// or earned ([`asked_wait`]). Returns how long the server is left alone.
    pub(crate) fn fail(&mut self, now: Instant, asked: Option<Duration>) -> Duration {
        let doubled = self.first.saturating_mul(1 << self.failures.min(16));
        let wait = match asked {
            Some(asked) => asked.min(LONGEST_ASKED).max(doubled.min(REFUSED_CAP)),
            None => doubled.min(self.cap),
        };
        self.failures = self.failures.saturating_add(1);
        self.until = Some(now + wait);
        wait
    }
}

/// The wait an HTTP failure asks for: `Retry-After` in seconds when the
/// answer carries one, and at least [`REFUSED`] for a 403 or a 429.
pub(crate) fn wait_for_status(code: u16, retry_after: Option<&str>) -> Option<Duration> {
    let named = retry_after
        .and_then(|text| text.trim().parse().ok())
        .map(Duration::from_secs);
    named.max(matches!(code, 403 | 429).then_some(REFUSED))
}

pub(crate) fn asked_wait(err: &ureq::Error) -> Option<Duration> {
    let ureq::Error::Status(code, response) = err else {
        return None;
    };
    wait_for_status(*code, response.header("retry-after"))
}

/// 403, 429, and a `Retry-After` named in an Esplora error string.
pub(crate) fn wait_from_message(err: &str) -> Option<Duration> {
    let code = if err.contains("HTTP 429") {
        429
    } else if err.contains("HTTP 403") {
        403
    } else {
        return err
            .split("Retry-After:")
            .nth(1)
            .and_then(|text| text.split_whitespace().next())
            .and_then(|text| text.parse().ok())
            .map(Duration::from_secs);
    };
    let named = err
        .split("Retry-After:")
        .nth(1)
        .and_then(|text| text.split_whitespace().next());
    wait_for_status(code, named)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn each_failure_doubles_the_wait_up_to_the_cap_and_an_answer_clears_it() {
        let start = Instant::now();
        let mut backoff = Backoff::new(30 * SECOND, 240 * SECOND);
        assert!(backoff.ready(start));
        let waits: Vec<u64> = (0..5)
            .map(|_| backoff.fail(start, None).as_secs())
            .collect();
        assert_eq!(waits, [30, 60, 120, 240, 240]);
        assert!(!backoff.ready(start + 239 * SECOND));
        assert!(backoff.ready(start + 240 * SECOND));
        backoff.ok();
        assert!(backoff.ready(start));
        assert_eq!(backoff.fail(start, None), 30 * SECOND);
    }

    #[test]
    fn a_server_that_refuses_is_left_alone_longer_and_at_most_an_hour_unless_it_says_so() {
        let start = Instant::now();
        let mut backoff = Backoff::new(30 * SECOND, 240 * SECOND);
        let minutes: Vec<u64> = (0..9)
            .map(|_| backoff.fail(start, Some(REFUSED)).as_secs() / 60)
            .collect();
        assert_eq!(minutes, [15, 15, 15, 15, 15, 16, 32, 60, 60]);
        assert_eq!(
            backoff.fail(start, Some(Duration::from_secs(7200))),
            Duration::from_secs(7200),
            "Retry-After is kept"
        );
        assert_eq!(
            backoff.fail(start, Some(Duration::from_secs(u64::MAX))),
            LONGEST_ASKED
        );
    }

    #[test]
    fn retry_after_and_refusals_are_read_from_the_answer() {
        let answer = |code, lines: &str| {
            ureq::Error::Status(
                code,
                format!("HTTP/1.1 {code} X\r\n{lines}\r\n").parse().unwrap(),
            )
        };
        assert_eq!(asked_wait(&answer(500, "")), None);
        assert_eq!(
            asked_wait(&answer(503, "Retry-After: 120\r\n")),
            Some(120 * SECOND)
        );
        assert_eq!(asked_wait(&answer(403, "")), Some(REFUSED));
        assert_eq!(
            asked_wait(&answer(429, "Retry-After: 5\r\n")),
            Some(REFUSED)
        );
        assert_eq!(
            asked_wait(&answer(429, "Retry-After: 3600\r\n")),
            Some(3600 * SECOND)
        );
        assert_eq!(
            asked_wait(&answer(
                429,
                "Retry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n"
            )),
            Some(REFUSED)
        );
        assert_eq!(wait_from_message("tip height HTTP 403"), Some(REFUSED));
        assert_eq!(
            wait_from_message("tip height HTTP 429 Retry-After: 1800"),
            Some(1800 * SECOND)
        );
        assert_eq!(wait_from_message("connection refused"), None);
    }
}
