//! Whether a phone can reach a new pairing code over Tor yet.
//!
//! Each code has its own onion service, as new as the code. A Tor client
//! finds it only once its descriptor is on the directories, and can fail to
//! connect for a while after that. So the owner's page holds the QR code
//! while the onion is [`Onion::Publishing`], and the node checks what it can
//! check from where it is: its own Tor client dials the new address the way
//! a phone does. It fetches the descriptor from the directories, meets the
//! service at a rendezvous point, and gets an HTTP answer from the pairing
//! gateway. Then the onion is [`Onion::Ready`].
//!
//! Arti's own report of the upload is logged and not waited for: it says
//! "published" only when every directory of both rings has answered, which
//! can be minutes after phones can connect.
//!
//! The check is the node's side of the network. A phone asks through other
//! relays and may ask other directories first, so it can still be told "not
//! found" or stall after the node has reached itself; the app keeps trying.
//! When the check has not passed [`SLOW_AFTER`] the code was made, the page
//! shows the QR anyway and says the address is slow ([`Onion::Slow`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arti_client::TorClient;
use tor_rtcompat::Runtime;
use tormail::{OnionAddress, OnionForward};

use super::pair_log::{line, scrub};
use super::tor::ADMIN_PORT;

/// How long the page holds the QR for an onion the node has not reached.
pub(crate) const SLOW_AFTER: Duration = Duration::from_secs(120);
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(45);
const CHECK_PAUSE: Duration = Duration::from_secs(3);
/// Any HTTP answer shows the whole path works. This one makes no request.
const CHECK: &[u8] = b"GET /pair/status/self-check HTTP/1.1\r\nHost: pairing\r\n\r\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Onion {
    Publishing,
    Ready,
    Slow,
}

impl Onion {
    /// As the page reads it in `POST /remote/pairing` and `GET /remote/pending`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Publishing => "publishing",
            Self::Ready => "ready",
            Self::Slow => "slow",
        }
    }
}

/// `reached`: the node's own Tor client got an answer from the onion.
/// `waited`: since the code was made.
pub(crate) fn state(reached: bool, waited: Duration) -> Onion {
    match reached {
        true => Onion::Ready,
        false if waited >= SLOW_AFTER => Onion::Slow,
        false => Onion::Publishing,
    }
}

/// Dial the pairing onion `forward` until it answers, then set `reached`.
/// Until then, log what Arti says about its descriptor each time that
/// changes. Ends when the onion has answered or the service is dropped.
pub(crate) async fn watch<R: Runtime>(
    client: Arc<TorClient<R>>,
    forward: OnionForward,
    label: String,
    reached: Arc<AtomicBool>,
) {
    let started = Instant::now();
    tokio::join!(
        report(&forward, &label, started, &reached),
        check(&client, &forward, &label, started, &reached),
    );
}

async fn report(forward: &OnionForward, label: &str, started: Instant, reached: &AtomicBool) {
    // A new service has not started. That is not worth a line.
    let mut seen = "Shutdown".to_string();
    while !reached.load(Ordering::Relaxed) {
        let state = forward.state();
        if state != seen {
            let after = started.elapsed().as_secs();
            let word = match state.as_str() {
                "Running" => "is on all its directories",
                "DegradedReachable" => "is on some of its directories",
                "Bootstrapping" => "is publishing",
                _ => "is not published",
            };
            // Arti reports an empty list of problems as `Ipt([])`.
            let problem = forward
                .problem()
                .filter(|problem| !problem.ends_with("([])"))
                .map(|problem| format!(": {}", scrub(&problem)))
                .unwrap_or_default();
            line(format!("{label} {word} after {after}s (Arti: {state}){problem}"));
            seen = state;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Dial `onion` once with the node's own Tor client, on circuits of its
/// own, so the descriptor is fetched from the directories again. `Ok` when
/// an HTTP answer came back within [`CHECK_TIMEOUT`].
pub(crate) async fn dial<R: Runtime>(client: &TorClient<R>, onion: &OnionAddress) -> Result<(), String> {
    let dial = tormail::exchange(client, onion, ADMIN_PORT, CHECK, 4096, true);
    match tokio::time::timeout(CHECK_TIMEOUT, dial).await {
        Ok(Ok(answer)) if answer.starts_with(b"HTTP/1.1 ") => Ok(()),
        Ok(Ok(_)) => Err("the answer was not HTTP".to_string()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("no answer in {}s", CHECK_TIMEOUT.as_secs())),
    }
}

async fn check<R: Runtime>(
    client: &TorClient<R>,
    forward: &OnionForward,
    label: &str,
    started: Instant,
    reached: &AtomicBool,
) {
    let Some(onion) = forward.onion_address() else {
        return;
    };
    let mut failed = String::new();
    for tries in 1.. {
        let failure = dial(client, &onion).await.err();
        let after = started.elapsed().as_secs();
        let Some(reason) = failure else {
            reached.store(true, Ordering::Relaxed);
            line(format!("{label} answered the node's own Tor client after {after}s (try {tries}); the page can show the code"));
            return;
        };
        // One line per reason, not per try.
        if reason != failed {
            line(format!("{label} did not answer the node's own Tor client (try {tries}, {after}s): {}", scrub(&reason)));
            failed = reason;
        }
        tokio::time::sleep(CHECK_PAUSE).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_is_held_until_the_onion_answers_or_two_minutes_pass() {
        let secs = Duration::from_secs;
        assert_eq!(state(false, secs(0)), Onion::Publishing);
        assert_eq!(state(false, SLOW_AFTER - secs(1)), Onion::Publishing);
        assert_eq!(state(false, SLOW_AFTER), Onion::Slow);
        assert_eq!(state(true, secs(20)), Onion::Ready);
        assert_eq!(state(true, SLOW_AFTER + secs(60)), Onion::Ready, "late is still ready");
        assert_eq!(
            [Onion::Publishing, Onion::Ready, Onion::Slow].map(Onion::name),
            ["publishing", "ready", "slow"]
        );
    }
}
