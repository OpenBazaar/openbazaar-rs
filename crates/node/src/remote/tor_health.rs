//! Whether the node still has useful contact with the Tor network, in plain
//! words: one console line when that changes, and `tor` in `GET /remote`.
//!
//! Arti says "connected" once it has bootstrapped and does not take it back
//! when connections start failing, and it goes on using a directory for
//! three days after the directory has expired. So a node whose Tor has
//! stalled runs on in silence. Every few seconds the node reads what Arti
//! does know ([`Reading`]) and sorts it into one [`State`]:
//!
//! * cut off: Arti is not connected or has no directory, or the directory
//!   (the consensus) ran out more than [`CUT_OFF_AFTER`] ago. A consensus
//!   is valid for three hours and Arti replaces it before the last ten
//!   minutes of them, so the node has then missed every refresh for over
//!   three hours.
//! * degraded: the directory ran out more than [`STALE_AFTER`] ago, or a
//!   long-lived onion service has not been published for
//!   [`UNPUBLISHED_AFTER`]. A new service is normally published within a
//!   few minutes, and Arti gives one upload five.
//! * starting: any of the above within [`GRACE`] of the start, or of the
//!   machine waking from sleep. Nothing is reported for it.
//! * fine: none of the above. An onion service may be unpublished for less
//!   than that quarter of an hour; the reason then says so.
//!
//! Arti can go on saying "not published" long after the uploads worked. So
//! the store's onion is not called unpublished while it has answered the
//! node's own Tor client in the last [`UNPUBLISHED_AFTER`]
//! ([`super::onion_check`]); the reason then says so too. That answer
//! cancels no other rule.
//!
//! The counts of the last hour ([`super::tor_counts`]) are for reading,
//! not for sorting: a hosted node logs about a hundred rounds of failed
//! circuits an hour while it works, so no rule rests on them.
//!
//! Onion services are named by role. No address or key is in here.

use serde_json::{json, Value};

use super::onion_check::{answered_ago, ROLE};
use super::pair_log::scrub;
use super::tor_counts::Tally;

/// After a start or a wake, how long trouble is "starting".
pub(crate) const GRACE: u64 = 10 * 60;
/// Past the directory's valid-until: degraded.
pub(crate) const STALE_AFTER: u64 = 30 * 60;
/// Past the directory's valid-until: cut off.
pub(crate) const CUT_OFF_AFTER: u64 = 3 * 3600;
/// A long-lived onion service not published for this long: degraded.
pub(crate) const UNPUBLISHED_AFTER: u64 = 15 * 60;
/// This much more time on the wall clock than on the clock that stops while
/// the machine sleeps, between two looks: the machine slept.
const SLEPT: u64 = 2 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Starting,
    Fine,
    Degraded,
    CutOff,
}

impl State {
    /// As the console and `GET /remote` say it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Fine => "fine",
            Self::Degraded => "degraded",
            Self::CutOff => "cut off",
        }
    }
}

/// The lifetime of the consensus Arti is using, in unix seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Directory {
    pub valid_after: u64,
    pub fresh_until: u64,
    pub valid_until: u64,
}

/// What Arti says at one moment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Reading {
    /// Arti has bootstrapped and still accepts its directory.
    pub ready: bool,
    /// Why Arti thinks it is stuck, when it does.
    pub blocked: Option<String>,
    pub directory: Option<Directory>,
    /// When the store's onion last answered the node's own Tor client.
    pub store_answered: Option<u64>,
    /// Each long-lived onion service: its role, Arti's state for it, and
    /// whether Arti believes it reachable.
    pub onions: Vec<(&'static str, String, bool)>,
}

struct Onion {
    role: &'static str,
    state: String,
    published: bool,
    /// Since when it has been published, or not.
    changed: u64,
    last_published: Option<u64>,
}

/// `45 s`, `12 min`, `57 h 3 min`
fn span(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

/// The state and its reason. `waited`: seconds since the start or the wake.
fn classify(now: u64, waited: u64, reading: &Reading, onions: &[Onion]) -> (State, String) {
    let expired = reading
        .directory
        .map_or(0, |directory| now.saturating_sub(directory.valid_until));
    let overdue = |onion: &&Onion| !onion.published && now.saturating_sub(onion.changed) >= UNPUBLISHED_AFTER;
    let answered = |onion: &Onion| answered_ago(now, reading.store_answered).filter(|_| onion.role == ROLE);
    let unpublished = onions
        .iter()
        .find(|onion| overdue(onion) && answered(onion).is_none());
    let trouble = if !reading.ready {
        let why = reading
            .blocked
            .as_deref()
            .map(|why| format!(": {}", scrub(why)))
            .unwrap_or_default();
        Some((State::CutOff, format!("Tor is not connected{why}.")))
    } else if reading.directory.is_none() {
        Some((
            State::CutOff,
            "The node has no usable copy of the Tor directory.".into(),
        ))
    } else if expired > CUT_OFF_AFTER {
        Some((State::CutOff, format!("The node's copy of the Tor directory expired {} ago and has not been replaced, so its onion addresses are probably unreachable.", span(expired))))
    } else if expired > STALE_AFTER {
        Some((State::Degraded, format!("The node's copy of the Tor directory expired {} ago and has not been replaced yet.", span(expired))))
    } else {
        unpublished.map(|onion| {
            let unpublished_for = span(now.saturating_sub(onion.changed));
            let unanswered = if onion.role == ROLE {
                format!(" and has not answered the node's own Tor client in the last {}", span(UNPUBLISHED_AFTER))
            } else {
                String::new()
            };
            (State::Degraded, format!("The {} onion address has not been published on Tor for {unpublished_for}{unanswered}, so it may be unreachable.", onion.role))
        })
    };
    let vouched = onions
        .iter()
        .filter(overdue)
        .find_map(|onion| Some((onion, answered(onion)?)));
    if let (None, Some((onion, ago))) = (&trouble, vouched) {
        return (State::Fine, format!("Tor is connected and its directory is current. Tor has not reported the {} onion address published for {}, but it answered the node's own Tor client {} ago.", onion.role, span(now.saturating_sub(onion.changed)), span(ago)));
    }
    match trouble {
        None => match onions.iter().find(|onion| !onion.published) {
            None => (State::Fine, "Tor is connected, its directory is current and the node's onion addresses are published.".into()),
            Some(onion) => (State::Fine, format!("Tor is connected and its directory is current. The {} onion address is being published.", onion.role)),
        },
        Some(_) if waited < GRACE => (State::Starting, "Tor is still starting.".into()),
        Some(trouble) => trouble,
    }
}

/// What the node has seen of Tor so far.
pub(crate) struct Tracker {
    /// When Tor started, or the machine last woke.
    since: u64,
    /// The last look: the time, and the clock that stops in sleep.
    seen: u64,
    awake: u64,
    connected_since: Option<u64>,
    onions: Vec<Onion>,
    state: State,
    state_since: u64,
    /// The last state the console was told: fine until it says otherwise.
    told: State,
    /// When the console was first told of the trouble that is going on.
    troubled: u64,
    summary: Option<Value>,
}

impl Tracker {
    pub(crate) const fn new() -> Self {
        Self {
            since: 0,
            seen: 0,
            awake: 0,
            connected_since: None,
            onions: Vec::new(),
            state: State::Starting,
            state_since: 0,
            told: State::Fine,
            troubled: 0,
            summary: None,
        }
    }

    /// `tor` in `GET /remote`. `None` before the first look.
    pub(crate) fn summary(&self) -> Option<Value> {
        self.summary.clone()
    }

    /// Take one look. `awake` is a clock in seconds that stops while the
    /// machine sleeps. Returns the console line when the state has changed
    /// to degraded, to cut off, or back to fine.
    pub(crate) fn observe(
        &mut self,
        now: u64,
        awake: u64,
        reading: &Reading,
        tally: &Tally,
    ) -> Option<String> {
        let first = self.seen == 0;
        let slept = now
            .saturating_sub(self.seen)
            .saturating_sub(awake.saturating_sub(self.awake));
        if first || slept > SLEPT {
            self.since = now;
            self.onions.iter_mut().for_each(|onion| onion.changed = now);
        }
        self.seen = now;
        self.awake = awake;
        self.connected_since = reading.ready.then(|| self.connected_since.unwrap_or(now));
        self.onions = reading
            .onions
            .iter()
            .map(|(role, state, published)| {
                let before = self.onions.iter().find(|onion| onion.role == *role);
                Onion {
                    role: *role,
                    state: state.clone(),
                    published: *published,
                    changed: before
                        .filter(|before| before.published == *published)
                        .map_or(now, |before| before.changed),
                    last_published: if *published {
                        Some(now)
                    } else {
                        before.and_then(|before| before.last_published)
                    },
                }
            })
            .collect();
        let (state, reason) = classify(now, now.saturating_sub(self.since), reading, &self.onions);
        if first || state != self.state {
            self.state = state;
            self.state_since = now;
        }
        let directory = reading.directory.map(|directory| {
            json!({
                "validAfter": directory.valid_after,
                "freshUntil": directory.fresh_until,
                "validUntil": directory.valid_until,
                "ageSecs": now.saturating_sub(directory.valid_after),
                "fresh": now <= directory.fresh_until,
                "valid": now <= directory.valid_until,
            })
        });
        let onions: Vec<Value> = self
            .onions
            .iter()
            .map(|onion| {
                let mut entry = json!({
                    "role": onion.role,
                    "published": onion.published,
                    "state": onion.state,
                    "lastPublished": onion.last_published,
                    "uploadsTimedOut": tally.timed_out_for(Some(onion.role)),
                });
                if onion.role == ROLE {
                    entry["lastAnswered"] = json!(reading.store_answered);
                }
                entry
            })
            .collect();
        self.summary = Some(json!({
            "state": state.name(),
            "reason": reason,
            "since": self.state_since,
            "checked": now,
            "connected": reading.ready,
            "connectedSince": self.connected_since,
            "blocked": reading.blocked.as_deref().map(scrub),
            "directory": directory,
            "onions": onions,
            "lastHour": {
                "uploadsTimedOut": tally.timed_out(),
                "uploadsRejected": tally.rejected,
                "circuitFailures": tally.circuits,
            },
        }));
        let word = match state {
            State::Degraded | State::CutOff if state != self.told => state.name().to_string(),
            State::Fine if self.told != State::Fine => format!(
                "fine again after {}",
                span(now.saturating_sub(self.troubled))
            ),
            _ => return None,
        };
        if self.told == State::Fine {
            self.troubled = now;
        }
        self.told = state;
        let age = reading.directory.map_or("none".to_string(), |directory| {
            format!("{} old", span(now.saturating_sub(directory.valid_after)))
        });
        let published = self.onions.iter().filter(|onion| onion.published).count();
        let unpublished: String = self
            .onions
            .iter()
            .filter(|onion| !onion.published)
            .map(|onion| format!(", {} is {}", onion.role, onion.state))
            .collect();
        Some(format!(
            "health: {word}: {reason} (directory {age}; onion addresses published {published} of {}{unpublished}; last hour: {} uploads timed out, {} rejected, {} rounds of failed circuits)",
            self.onions.len(),
            tally.timed_out(),
            tally.rejected,
            tally.circuits,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday 8 November 2026, 15:00 UTC: the valid-after of the consensus a
    /// node of the owner's was found still using 57 hours later.
    const T: u64 = 1_794_150_000;
    const HOUR: u64 = 3600;
    const MIN: u64 = 60;

    fn directory(valid_after: u64) -> Option<Directory> {
        Some(Directory {
            valid_after,
            fresh_until: valid_after + HOUR,
            valid_until: valid_after + 3 * HOUR,
        })
    }

    fn reading(valid_after: u64, store: &str) -> Reading {
        Reading {
            ready: true,
            blocked: None,
            directory: directory(valid_after),
            store_answered: None,
            onions: vec![
                ("remote access", "Running".into(), true),
                (
                    "store",
                    store.into(),
                    matches!(store, "Running" | "DegradedReachable"),
                ),
            ],
        }
    }

    fn state_of(tracker: &Tracker) -> (String, String) {
        let summary = tracker.summary().unwrap();
        (
            summary["state"].as_str().unwrap().to_string(),
            summary["reason"].as_str().unwrap().to_string(),
        )
    }

    /// The state after one look at `reading`, `waited` seconds after the start.
    fn sorted(waited: u64, now: u64, reading: &Reading) -> State {
        let onions: Vec<Onion> = reading
            .onions
            .iter()
            .map(|(role, state, published)| Onion {
                role: *role,
                state: state.clone(),
                published: *published,
                changed: now - waited,
                last_published: None,
            })
            .collect();
        classify(now, waited, reading, &onions).0
    }

    #[test]
    fn the_directory_rules_follow_the_consensus_times() {
        let good = reading(T, "Running");
        let long_up = 24 * HOUR;
        // A consensus is used past its fresh-until every hour of its life: that is fine.
        assert_eq!(sorted(long_up, T + 2 * HOUR + 50 * MIN, &good), State::Fine);
        // Just expired: Arti is at most a little late.
        assert_eq!(
            sorted(long_up, T + 3 * HOUR + STALE_AFTER, &good),
            State::Fine
        );
        assert_eq!(
            sorted(long_up, T + 3 * HOUR + STALE_AFTER + 1, &good),
            State::Degraded
        );
        assert_eq!(
            sorted(long_up, T + 3 * HOUR + CUT_OFF_AFTER, &good),
            State::Degraded
        );
        assert_eq!(
            sorted(long_up, T + 3 * HOUR + CUT_OFF_AFTER + 1, &good),
            State::CutOff
        );
        // The node that ran 57 hours on one consensus was cut off from the 6th hour.
        assert_eq!(sorted(long_up, T + 57 * HOUR, &good), State::CutOff);
        let (_, reason) = classify(T + 57 * HOUR, long_up, &good, &[]);
        assert_eq!(reason, "The node's copy of the Tor directory expired 54 h 0 min ago and has not been replaced, so its onion addresses are probably unreachable.");
        // The store answering the node's own Tor client cancels none of this:
        // that dial uses the same directory.
        let answering = |now: u64| Reading {
            store_answered: Some(now),
            ..reading(T, "Bootstrapping")
        };
        let stale = T + 3 * HOUR + STALE_AFTER;
        assert_eq!(sorted(long_up, stale, &answering(stale)), State::Fine);
        assert_eq!(sorted(long_up, stale + 1, &answering(stale + 1)), State::Degraded);
        let gone = T + 3 * HOUR + CUT_OFF_AFTER + 1;
        assert_eq!(sorted(long_up, gone, &answering(gone)), State::CutOff);
        let (_, reason) = classify(stale + 1, long_up, &answering(stale + 1), &[]);
        assert_eq!(reason, "The node's copy of the Tor directory expired 30 min ago and has not been replaced yet.");
        let offline = Reading {
            ready: false,
            ..answering(T)
        };
        assert_eq!(sorted(long_up, T, &offline), State::CutOff);
    }

    #[test]
    fn no_connection_is_cut_off_and_says_what_arti_says() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        let offline = Reading {
            ready: false,
            blocked: Some(format!(
                "We seem to be offline (unable to connect to the internet) {onion}.onion"
            )),
            ..Reading::default()
        };
        let (state, reason) = classify(T, GRACE, &offline, &[]);
        assert_eq!(state, State::CutOff);
        assert_eq!(reason, "Tor is not connected: We seem to be offline (unable to connect to the internet) 25njqa…5sid.onion.");
        let no_directory = Reading {
            directory: None,
            ..reading(T, "Running")
        };
        assert_eq!(
            classify(T, GRACE, &no_directory, &[]),
            (
                State::CutOff,
                "The node has no usable copy of the Tor directory.".into()
            )
        );
    }

    #[test]
    fn an_onion_must_stay_unpublished_for_a_quarter_of_an_hour() {
        let down = reading(T, "Recovering");
        assert_eq!(sorted(UNPUBLISHED_AFTER - 1, T + HOUR, &down), State::Fine);
        assert_eq!(sorted(UNPUBLISHED_AFTER, T + HOUR, &down), State::Degraded);
        let onions = [Onion {
            role: "store",
            state: "Recovering".into(),
            published: false,
            changed: T,
            last_published: None,
        }];
        let (_, reason) = classify(T + 20 * MIN, HOUR, &down, &onions);
        assert_eq!(reason, "The store onion address has not been published on Tor for 20 min and has not answered the node's own Tor client in the last 15 min, so it may be unreachable.");
    }

    #[test]
    fn the_store_is_degraded_only_when_it_has_not_answered_the_node_either() {
        let now = T + HOUR;
        let doubted = |answered: Option<u64>| Reading {
            store_answered: answered,
            ..reading(T, "Bootstrapping")
        };
        let recent = Some(now - UNPUBLISHED_AFTER + 1);
        let old = Some(now - UNPUBLISHED_AFTER);
        assert_eq!(sorted(UNPUBLISHED_AFTER, now, &doubted(recent)), State::Fine);
        assert_eq!(sorted(3 * HOUR, now, &doubted(recent)), State::Fine);
        assert_eq!(sorted(UNPUBLISHED_AFTER, now, &doubted(old)), State::Degraded);
        assert_eq!(sorted(UNPUBLISHED_AFTER, now, &doubted(None)), State::Degraded);
        assert_eq!(sorted(UNPUBLISHED_AFTER - 1, now, &doubted(None)), State::Fine);

        let onion = |role, changed| Onion {
            role,
            state: "Bootstrapping".into(),
            published: false,
            changed,
            last_published: None,
        };
        let store = [onion("store", now - 35 * MIN)];
        let (state, reason) = classify(now, HOUR, &doubted(Some(now - 2 * MIN)), &store);
        assert_eq!(state, State::Fine);
        assert_eq!(reason, "Tor is connected and its directory is current. Tor has not reported the store onion address published for 35 min, but it answered the node's own Tor client 2 min ago.");
        let (state, reason) = classify(now, HOUR, &doubted(Some(now - 16 * MIN)), &store);
        assert_eq!(state, State::Degraded);
        assert_eq!(reason, "The store onion address has not been published on Tor for 35 min and has not answered the node's own Tor client in the last 15 min, so it may be unreachable.");

        // Only the store is dialled: its answer says nothing of remote access.
        let both = [onion("remote access", now - 20 * MIN), onion("store", now - 35 * MIN)];
        let (state, reason) = classify(now, HOUR, &doubted(Some(now - 2 * MIN)), &both);
        assert_eq!(state, State::Degraded);
        assert_eq!(reason, "The remote access onion address has not been published on Tor for 20 min, so it may be unreachable.");
    }

    #[test]
    fn a_start_and_a_wake_get_ten_minutes_before_anything_is_said() {
        let mut tracker = Tracker::new();
        let tally = Tally::default();
        let offline = Reading::default();
        let mut lines = Vec::new();
        // Starting with no connection, then connected with a cached, expired directory.
        lines.extend(tracker.observe(T + 9 * HOUR, 0, &offline, &tally));
        assert_eq!(
            state_of(&tracker),
            ("starting".into(), "Tor is still starting.".into())
        );
        lines.extend(tracker.observe(
            T + 9 * HOUR + MIN,
            MIN,
            &reading(T, "Bootstrapping"),
            &tally,
        ));
        assert_eq!(state_of(&tracker).0, "starting");
        // A new directory arrives and the onions are published: fine, and not worth a line.
        let start = T + 9 * HOUR + 2 * MIN;
        lines.extend(tracker.observe(
            start,
            2 * MIN,
            &reading(start - 20 * MIN, "Running"),
            &tally,
        ));
        assert_eq!(state_of(&tracker).0, "fine");
        assert_eq!(lines, Vec::<String>::new());
        // The machine sleeps for eight hours. Its directory is stale when it
        // wakes, and Tor gets the same ten minutes to replace it.
        let wake = start + 8 * HOUR;
        for late in (0..GRACE).step_by(5) {
            lines.extend(tracker.observe(
                wake + late,
                2 * MIN + late,
                &reading(start - 20 * MIN, "Recovering"),
                &tally,
            ));
            assert_eq!(state_of(&tracker).0, "starting", "{late}s after waking");
        }
        assert_eq!(lines, Vec::<String>::new());
        lines.extend(tracker.observe(
            wake + GRACE,
            2 * MIN + GRACE,
            &reading(start - 20 * MIN, "Recovering"),
            &tally,
        ));
        assert_eq!(state_of(&tracker).0, "cut off");
        assert_eq!(lines.len(), 1);
        // A look that comes late because the node was busy is not a wake.
        lines.extend(tracker.observe(
            wake + GRACE + HOUR,
            2 * MIN + GRACE + HOUR,
            &reading(start - 20 * MIN, "Recovering"),
            &tally,
        ));
        assert_eq!(state_of(&tracker).0, "cut off");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn the_console_is_told_once_per_change() {
        let mut tracker = Tracker::new();
        let tally = Tally {
            timed_out: vec![(Some("store"), 14), (None, 2)],
            rejected: 3,
            circuits: 80,
        };
        let mut lines = Vec::new();
        // One look every five seconds for nine hours, on a consensus that is never replaced.
        for now in (T..T + 9 * HOUR).step_by(5) {
            lines.extend(tracker.observe(now, now, &reading(T, "Running"), &tally));
        }
        // A new consensus at last, and the store's onion drops out for a while.
        let back = T + 9 * HOUR;
        for now in (back..back + HOUR).step_by(5) {
            let store = if (back + 10 * MIN..back + 40 * MIN).contains(&now) {
                "Recovering"
            } else {
                "Running"
            };
            lines.extend(tracker.observe(now, now, &reading(back - 30 * MIN, store), &tally));
        }
        let numbers = "onion addresses published 2 of 2; last hour: 16 uploads timed out, 3 rejected, 80 rounds of failed circuits)";
        assert_eq!(
            lines,
            [
                format!("health: degraded: The node's copy of the Tor directory expired 30 min ago and has not been replaced yet. (directory 3 h 30 min old; {numbers}"),
                format!("health: cut off: The node's copy of the Tor directory expired 3 h 0 min ago and has not been replaced, so its onion addresses are probably unreachable. (directory 6 h 0 min old; {numbers}"),
                format!("health: fine again after 5 h 29 min: Tor is connected, its directory is current and the node's onion addresses are published. (directory 30 min old; {numbers}"),
                "health: degraded: The store onion address has not been published on Tor for 15 min and has not answered the node's own Tor client in the last 15 min, so it may be unreachable. (directory 55 min old; onion addresses published 1 of 2, store is Recovering; last hour: 16 uploads timed out, 3 rejected, 80 rounds of failed circuits)".to_string(),
                format!("health: fine again after 15 min: Tor is connected, its directory is current and the node's onion addresses are published. (directory 1 h 10 min old; {numbers}"),
            ]
        );
    }

    #[test]
    fn the_summary_names_roles_and_no_address() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        let mut tracker = Tracker::new();
        assert_eq!(tracker.summary(), None);
        let now = T + 2 * HOUR;
        let look = Reading {
            blocked: Some(format!("could not reach {onion}.onion")),
            store_answered: Some(now - 30),
            ..reading(T, "DegradedUnreachable")
        };
        let tally = Tally {
            timed_out: vec![(Some("store"), 7), (None, 2)],
            rejected: 3,
            circuits: 26,
        };
        tracker.observe(now - MIN, 0, &reading(T, "Running"), &tally);
        tracker.observe(now, MIN, &look, &tally);
        let summary = tracker.summary().unwrap();
        assert_eq!(
            summary,
            json!({
                "state": "fine",
                "reason": "Tor is connected and its directory is current. The store onion address is being published.",
                "since": now - MIN,
                "checked": now,
                "connected": true,
                "connectedSince": now - MIN,
                "blocked": "could not reach 25njqa…5sid.onion",
                "directory": { "validAfter": T, "freshUntil": T + HOUR, "validUntil": T + 3 * HOUR, "ageSecs": 2 * HOUR, "fresh": false, "valid": true },
                "onions": [
                    { "role": "remote access", "published": true, "state": "Running", "lastPublished": now, "uploadsTimedOut": 0 },
                    { "role": "store", "published": false, "state": "DegradedUnreachable", "lastPublished": now - MIN, "uploadsTimedOut": 7, "lastAnswered": now - 30 },
                ],
                "lastHour": { "uploadsTimedOut": 9, "uploadsRejected": 3, "circuitFailures": 26 },
            })
        );
        assert!(!summary.to_string().contains(onion));
    }
}
