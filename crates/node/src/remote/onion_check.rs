//! A second opinion on whether the store's onion address can be reached.
//!
//! Arti says an onion service is published from its own bookkeeping, and
//! that can stay at "not published" for an hour after every upload has
//! worked. So while Arti does not say published, the node's own Tor client
//! dials the store's address the way a buyer's does
//! ([`super::pair_ready::dial`]): it fetches the descriptor from the
//! directories again, meets the service at a rendezvous point, and gets an
//! HTTP answer from the sale gate. An answer in the last
//! [`UNPUBLISHED_AFTER`] keeps [`super::tor_health`] from calling the
//! store's address unpublished.
//!
//! The dial uses the node's own copy of the Tor directory, so it says
//! nothing about a directory that has gone stale: those rules come first
//! and an answer does not cancel them. Nothing is dialled while Arti says
//! published.
//!
//! Only the store is dialled. The remote-access address is open to paired
//! devices only, and the node holds no key to find it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arti_client::TorClient;
use tor_rtcompat::Runtime;
use tormail::OnionForward;

use super::now_secs;
use super::pair_log::scrub;
use super::pair_ready::dial;
use super::tor_health::UNPUBLISHED_AFTER;
use super::tor_log::print;

/// The role of the one onion service that is dialled.
pub(crate) const ROLE: &str = "store";
/// Arti has not said published for this long: the first dial.
const FIRST_DIAL: u64 = 2 * 60;
/// From the start of one dial to the start of the next.
const DIAL_EVERY: u64 = 4 * 60;
const LOOK: Duration = Duration::from_secs(5);

/// How long ago the store's onion last answered, when that is recent enough
/// to count. `answered`: when it did, in unix seconds.
pub(crate) fn answered_ago(now: u64, answered: Option<u64>) -> Option<u64> {
    answered
        .map(|at| now.saturating_sub(at))
        .filter(|ago| *ago < UNPUBLISHED_AFTER)
}

/// When the store's onion last answered: the watch writes it, the health
/// check reads it.
#[derive(Clone, Default)]
pub(crate) struct Answered(Arc<AtomicU64>);

impl Answered {
    /// In unix seconds. `None` when it never has.
    pub(crate) fn at(&self) -> Option<u64> {
        Some(self.0.load(Ordering::Relaxed)).filter(|at| *at > 0)
    }
}

/// When to dial. Times are seconds on any one clock.
#[derive(Default)]
struct Schedule {
    /// Since when Arti has not said published.
    doubted: Option<u64>,
    /// When the last dial of this doubt began.
    dialled: Option<u64>,
}

impl Schedule {
    /// One look. True when a dial is due now; it then counts as begun.
    fn look(&mut self, now: u64, published: bool) -> bool {
        if published {
            *self = Self::default();
            return false;
        }
        let doubted = *self.doubted.get_or_insert(now);
        let due = match self.dialled {
            None => now.saturating_sub(doubted) >= FIRST_DIAL,
            Some(dialled) => now.saturating_sub(dialled) >= DIAL_EVERY,
        };
        if due {
            self.dialled = Some(now);
        }
        due
    }
}

/// Dial the store onion `forward` while Arti does not say it is published,
/// and keep the time of the last answer in `answered`. Says on the console
/// how a dial went when that differs from the one before. Ends when the
/// service is dropped.
pub(crate) async fn watch<R: Runtime>(
    client: Arc<TorClient<R>>,
    forward: OnionForward,
    answered: Answered,
) {
    let started = Instant::now();
    let mut schedule = Schedule::default();
    let mut said = None;
    loop {
        tokio::time::sleep(LOOK).await;
        let published = forward.is_published();
        if published {
            said = None;
        }
        if !schedule.look(started.elapsed().as_secs(), published) {
            continue;
        }
        let Some(onion) = forward.onion_address() else {
            continue;
        };
        let went = dial(&client, &onion).await;
        if went.is_ok() {
            answered.0.store(now_secs(), Ordering::Relaxed);
        }
        if said.as_ref() != Some(&went) {
            print(line(&went, &forward.state()));
            said = Some(went);
        }
    }
}

/// The console line for one dial. `state`: Arti's name for the service's state.
fn line(went: &Result<(), String>, state: &str) -> String {
    match went {
        Ok(()) => format!("self-check: the {ROLE} onion address answered the node's own Tor client (Arti: {state})"),
        Err(reason) => format!("self-check: the {ROLE} onion address did not answer the node's own Tor client (Arti: {state}): {}", scrub(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::pair_ready::CHECK_TIMEOUT;

    /// The seconds at which a dial begins, with one look every [`LOOK`].
    fn dials(schedule: &mut Schedule, looks: std::ops::Range<u64>, published: bool) -> Vec<u64> {
        looks
            .step_by(LOOK.as_secs() as usize)
            .filter(|now| schedule.look(*now, published))
            .collect()
    }

    #[test]
    fn the_first_dial_is_at_two_minutes_of_doubt_and_then_one_every_four() {
        const MIN: u64 = 60;
        let mut schedule = Schedule::default();
        // Nothing is dialled while Arti says published, however long.
        assert_eq!(dials(&mut schedule, 0..60 * MIN, true), Vec::<u64>::new());
        // Arti stops saying so on the hour.
        let doubt = 60 * MIN;
        assert_eq!(
            dials(&mut schedule, doubt..doubt + 20 * MIN, false),
            [2, 6, 10, 14, 18].map(|min| doubt + min * MIN)
        );
        // Published again: no more dials, and the next doubt starts over.
        let back = doubt + 20 * MIN;
        assert_eq!(dials(&mut schedule, back..back + 30 * MIN, true), Vec::<u64>::new());
        let again = back + 30 * MIN;
        assert_eq!(
            dials(&mut schedule, again..again + 7 * MIN, false),
            [again + 2 * MIN, again + 6 * MIN]
        );
        // A doubt shorter than two minutes is never dialled.
        let mut schedule = Schedule::default();
        assert_eq!(dials(&mut schedule, 0..2 * MIN, false), Vec::<u64>::new());
        assert_eq!(dials(&mut schedule, 2 * MIN..9 * MIN, true), Vec::<u64>::new());
    }

    #[test]
    fn a_dial_gets_45_seconds_and_is_over_before_the_next() {
        assert_eq!(CHECK_TIMEOUT, Duration::from_secs(45));
        assert!(CHECK_TIMEOUT + LOOK < Duration::from_secs(DIAL_EVERY));
        // Four dials fit in the quarter of an hour before the node would say degraded.
        assert!(FIRST_DIAL + 3 * DIAL_EVERY + CHECK_TIMEOUT.as_secs() < UNPUBLISHED_AFTER);
    }

    #[test]
    fn an_answer_counts_for_a_quarter_of_an_hour() {
        let now = 1_794_150_000;
        assert_eq!(answered_ago(now, None), None);
        assert_eq!(answered_ago(now, Some(now)), Some(0));
        assert_eq!(answered_ago(now, Some(now - UNPUBLISHED_AFTER + 1)), Some(UNPUBLISHED_AFTER - 1));
        assert_eq!(answered_ago(now, Some(now - UNPUBLISHED_AFTER)), None);
    }

    #[test]
    fn the_console_line_has_the_state_and_no_address() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        assert_eq!(
            line(&Ok(()), "Bootstrapping"),
            "self-check: the store onion address answered the node's own Tor client (Arti: Bootstrapping)"
        );
        let failed = line(&Err(format!("could not reach {onion}.onion")), "Recovering");
        assert_eq!(failed, "self-check: the store onion address did not answer the node's own Tor client (Arti: Recovering): could not reach 25njqa…5sid.onion");
        assert!(!failed.contains(onion));
    }
}
