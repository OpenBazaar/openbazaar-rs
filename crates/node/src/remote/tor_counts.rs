//! Three of Arti's warnings, counted over the last hour for
//! [`super::tor_health`]: uploads of an onion service's descriptor that Arti
//! gave up on after five minutes of retries, uploads a directory rejected
//! with status 400 (it most likely held that descriptor or a newer one
//! already), and rounds of failed onion-service circuits.
//!
//! Arti keeps no such counters and exposes none of this as status, so the
//! wording of its warnings is matched ([`event`]). A rejection names no
//! service, so that count is for the whole node.

use std::sync::Mutex;

use super::services::role;

/// The warnings Arti has logged. One Arti log per process.
pub(crate) static COUNTS: Mutex<Counts> = Mutex::new(Counts::new());

/// One of the Arti warnings that are counted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event<'a> {
    /// An upload Arti gave up on after five minutes of retries, and the
    /// service's nickname.
    TimedOut(&'a str),
    /// An upload a directory answered with status 400.
    Rejected,
    /// A round of onion-service circuits that failed.
    Circuits,
}

/// Which counted event a warning is: `target` is the Arti module that logged
/// it, `text` its message and fields as [`super::tor_log`] prints them. Arti
/// exposes none of these as status, so its wording is matched: the module,
/// the start of the message, and the error.
pub(crate) fn event<'a>(target: &str, text: &'a str) -> Option<Event<'a>> {
    match target {
        "tor_hsservice::publish::reactor" => {
            if let Some(rest) = text.strip_prefix("failed to upload descriptor for service ") {
                let nickname = rest.split(' ').next().unwrap_or_default();
                return rest
                    .contains("Timeout exceeded")
                    .then_some(Event::TimedOut(nickname));
            }
            let rejected = text
                .starts_with("The peer says our HS descriptor upload request was invalid")
                && text.contains("HTTP status code 400");
            rejected.then_some(Event::Rejected)
        }
        "tor_circmgr::hspool" => text
            .starts_with("Too many preemptive onion service circuits failed")
            .then_some(Event::Circuits),
        _ => None,
    }
}

/// How many times something happened in the last hour, by the minute.
struct Hour {
    minutes: [u32; 60],
    /// The minute last counted in.
    minute: u64,
}

impl Hour {
    const fn new() -> Self {
        Self {
            minutes: [0; 60],
            minute: 0,
        }
    }

    /// The count, after forgetting the minutes that are over an hour old.
    fn count(&mut self, now: u64) -> u32 {
        let minute = now / 60;
        for passed in self.minute + 1..=minute.min(self.minute + 60) {
            self.minutes[(passed % 60) as usize] = 0;
        }
        self.minute = self.minute.max(minute);
        self.minutes.iter().sum()
    }

    fn add(&mut self, now: u64) -> u32 {
        self.count(now);
        self.minutes[(now / 60 % 60) as usize] += 1;
        self.minutes.iter().sum()
    }
}

/// The counted warnings of the last hour.
pub(crate) struct Counts {
    /// By the role of the service; `None` for pairing and reply onions.
    timed_out: Vec<(Option<&'static str>, Hour)>,
    rejected: Hour,
    circuits: Hour,
}

impl Counts {
    pub(crate) const fn new() -> Self {
        Self {
            timed_out: Vec::new(),
            rejected: Hour::new(),
            circuits: Hour::new(),
        }
    }

    /// Count the warning if it is a counted one. True for a rejection that
    /// is not the first of its hour: the count says it, another line need not.
    pub(crate) fn note(&mut self, target: &str, text: &str, now: u64) -> bool {
        match event(target, text) {
            Some(Event::TimedOut(nickname)) => {
                let role = role(nickname);
                if !self.timed_out.iter().any(|(known, _)| *known == role) {
                    self.timed_out.push((role, Hour::new()));
                }
                for (_, hour) in self
                    .timed_out
                    .iter_mut()
                    .filter(|(known, _)| *known == role)
                {
                    hour.add(now);
                }
                false
            }
            Some(Event::Rejected) => self.rejected.add(now) > 1,
            Some(Event::Circuits) => {
                self.circuits.add(now);
                false
            }
            None => false,
        }
    }

    pub(crate) fn tally(&mut self, now: u64) -> Tally {
        Tally {
            timed_out: self
                .timed_out
                .iter_mut()
                .map(|(role, hour)| (*role, hour.count(now)))
                .collect(),
            rejected: self.rejected.count(now),
            circuits: self.circuits.count(now),
        }
    }
}

/// [`Counts`] at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub timed_out: Vec<(Option<&'static str>, u32)>,
    pub rejected: u32,
    pub circuits: u32,
}

impl Tally {
    pub(crate) fn timed_out(&self) -> u32 {
        self.timed_out.iter().map(|(_, count)| count).sum()
    }

    pub(crate) fn timed_out_for(&self, role: Option<&str>) -> u32 {
        self.timed_out
            .iter()
            .filter(|(known, _)| *known == role)
            .map(|(_, count)| count)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_794_150_000;
    const HOUR: u64 = 3600;
    const MIN: u64 = 60;

    #[test]
    fn only_the_three_warnings_are_counted_and_only_for_an_hour() {
        let publisher = "tor_hsservice::publish::reactor";
        let timed_out = "failed to upload descriptor for service ob-sale (hsdir_id=$aa, hsdir_rsa_id=$bb) error=Timeout exceeded";
        let rejected = "The peer says our HS descriptor upload request was invalid. This is a bug (maybe theirs; maybe ours): error: descriptor upload request failed: HTTP status code 400: \"Invalid HS descriptor. Rejected.\"";
        let circuits = "Too many preemptive onion service circuits failed; waiting a while.";
        assert_eq!(
            event(publisher, timed_out),
            Some(Event::TimedOut("ob-sale"))
        );
        assert_eq!(event(publisher, rejected), Some(Event::Rejected));
        assert_eq!(
            event("tor_circmgr::hspool", circuits),
            Some(Event::Circuits)
        );
        // Another error, another status, another module.
        assert_eq!(event(publisher, "failed to upload descriptor for service ob-sale (hsdir_id=$aa) error=Internal error"), None);
        assert_eq!(event(publisher, &rejected.replace("400", "404")), None);
        assert_eq!(event("tor_circmgr::hspool", timed_out), None);
        assert_eq!(event("hyper::client", circuits), None);

        let mut counts = Counts::new();
        assert!(!counts.note(publisher, timed_out, T));
        assert!(!counts.note(
            publisher,
            &timed_out.replace("ob-sale", "ob-home-2"),
            T + 10 * MIN
        ));
        assert!(!counts.note(
            publisher,
            &timed_out.replace("ob-sale", "ob-pair-3fa9c1b2d4e6"),
            T + 10 * MIN
        ));
        assert!(
            !counts.note(publisher, rejected, T + 10 * MIN),
            "the first rejection of an hour is printed"
        );
        assert!(
            counts.note(publisher, rejected, T + 20 * MIN),
            "the next ones are only counted"
        );
        assert!(!counts.note("tor_circmgr::hspool", circuits, T + 30 * MIN));
        assert!(!counts.note(
            "tor_guardmgr::guard",
            "Could not connect to guard",
            T + 30 * MIN
        ));
        let all = Tally {
            timed_out: vec![(Some("store"), 1), (Some("remote access"), 1), (None, 1)],
            rejected: 2,
            circuits: 1,
        };
        assert_eq!(counts.tally(T + 59 * MIN), all);
        assert_eq!(all.timed_out(), 3);
        assert_eq!(all.timed_out_for(Some("remote access")), 1);
        // An hour after each, it is gone.
        assert_eq!(
            counts.tally(T + 60 * MIN).timed_out,
            [(Some("store"), 0), (Some("remote access"), 1), (None, 1)]
        );
        assert_eq!(
            counts.tally(T + 80 * MIN),
            Tally {
                timed_out: vec![(Some("store"), 0), (Some("remote access"), 0), (None, 0)],
                rejected: 0,
                circuits: 1
            }
        );
        assert!(
            !counts.note(publisher, rejected, T + 80 * MIN),
            "a new hour, a new line"
        );
        assert_eq!(counts.tally(T + 3 * HOUR).circuits, 0);
    }
}
