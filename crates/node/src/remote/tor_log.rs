//! Arti's warnings and errors, on the node's console.
//!
//! Arti reports through `tracing`, and with nothing listening those reports
//! are dropped. This listens for warnings and errors from Arti's crates only
//! and prints each as `ob-node: tor: …`, rate-limited. Arti scrubs addresses
//! and keys from what it logs (`safelog`), and whole onion names are
//! shortened here as well. Three kinds of warning are also counted
//! ([`super::tor_counts`]).

use std::fmt::{Debug, Write};
use std::sync::Mutex;
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use super::now_secs;
use super::pair_log::{scrub, Limit};
use super::tor_counts::{Counts, COUNTS};

struct Console {
    limit: Mutex<Limit>,
    counts: &'static Mutex<Counts>,
}

/// The event's message, then its other fields as ` name=value`.
struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            self.0.insert_str(0, &format!("{value:?}"));
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

impl Subscriber for Console {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let target = metadata.target();
        *metadata.level() <= Level::WARN && (target.starts_with("tor_") || target.starts_with("arti_"))
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::WARN)
    }

    fn event(&self, event: &Event<'_>) {
        let mut text = Text(String::new());
        event.record(&mut text);
        let metadata = event.metadata();
        // Every warning is counted for the health summary, shown or not.
        // A directory's rejection is shown once an hour.
        let repeat = self
            .counts
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .note(metadata.target(), &text.0, now_secs());
        if repeat {
            return;
        }
        let allowed = self
            .limit
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .allow(Instant::now());
        let Some(dropped) = allowed else {
            return;
        };
        if dropped > 0 {
            print(format!("{dropped} more warnings were not shown"));
        }
        print(format!("{} {}: {}", metadata.level(), metadata.target(), scrub(&text.0)));
    }

    // Spans carry nothing this prints.
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

pub(super) fn print(text: String) {
    #[cfg(test)]
    tests::LINES.with_borrow_mut(|lines| lines.push(text.clone()));
    eprintln!("ob-node: tor: {text}");
}

/// Print Arti's warnings and errors from now on. Once per process; a second
/// call changes nothing.
pub(crate) fn install() {
    let _ = tracing::subscriber::set_global_default(Console {
        limit: Mutex::new(Limit::new()),
        counts: &COUNTS,
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        pub(super) static LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn console() -> (Console, &'static Mutex<Counts>) {
        let counts: &'static Mutex<Counts> = Box::leak(Box::new(Mutex::new(Counts::new())));
        let console = Console {
            limit: Mutex::new(Limit::new()),
            counts,
        };
        (console, counts)
    }

    #[test]
    fn a_rejected_upload_is_printed_once_and_counted_every_time() {
        let (console, counts) = console();
        let rejected = "The peer says our HS descriptor upload request was invalid. \
            This is a bug (maybe theirs; maybe ours): error: descriptor upload request failed: \
            HTTP status code 400: \"Invalid HS descriptor. Rejected.\"";
        let timed_out = "failed to upload descriptor for service ob-sale (hsdir_id=$aa)";
        tracing::subscriber::with_default(console, || {
            for _ in 0..3 {
                tracing::warn!(target: "tor_hsservice::publish::reactor", "{rejected}");
                tracing::warn!(
                    target: "tor_hsservice::publish::reactor",
                    error = %"Timeout exceeded",
                    "{timed_out}"
                );
            }
        });
        let lines = LINES.take();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(
            lines[0],
            format!("WARN tor_hsservice::publish::reactor: {rejected}")
        );
        let gave_up = format!("WARN tor_hsservice::publish::reactor: {timed_out} error=Timeout exceeded");
        assert_eq!(lines[1..], [gave_up.clone(), gave_up.clone(), gave_up]);
        let tally = counts.lock().unwrap().tally(now_secs());
        assert_eq!(
            (tally.rejected, tally.timed_out),
            (3, vec![(Some("store"), 3)])
        );
    }

    #[test]
    fn arti_warnings_are_printed_and_nothing_else() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        let (console, _) = console();
        tracing::subscriber::with_default(console, || {
            tracing::warn!(target: "tor_hsservice::publish", attempt = 2, "could not upload the descriptor of {onion}.onion");
            tracing::error!(target: "arti_client::client", "bootstrap failed");
            tracing::info!(target: "tor_dirmgr", "an ordinary report");
            tracing::error!(target: "hyper::client", "not from Arti");
        });
        assert_eq!(
            LINES.take(),
            [
                "WARN tor_hsservice::publish: could not upload the descriptor of 25njqa…5sid.onion attempt=2",
                "ERROR arti_client::client: bootstrap failed",
            ]
        );
    }
}
