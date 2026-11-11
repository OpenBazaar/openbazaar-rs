//! Arti's warnings and errors, on the node's console.
//!
//! Arti reports through `tracing`, and with nothing listening those reports
//! are dropped. This listens for warnings and errors from Arti's crates only
//! and prints each as `ob-node: tor: …`, rate-limited. Arti scrubs addresses
//! and keys from what it logs (`safelog`), and whole onion names are
//! shortened here as well.

use std::fmt::{Debug, Write};
use std::sync::Mutex;
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use super::pair_log::{scrub, Limit};

struct Console {
    limit: Mutex<Limit>,
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
        let mut text = Text(String::new());
        event.record(&mut text);
        let metadata = event.metadata();
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

fn print(text: String) {
    #[cfg(test)]
    tests::LINES.with_borrow_mut(|lines| lines.push(text.clone()));
    eprintln!("ob-node: tor: {text}");
}

/// Print Arti's warnings and errors from now on. Once per process; a second
/// call changes nothing.
pub(crate) fn install() {
    let _ = tracing::subscriber::set_global_default(Console {
        limit: Mutex::new(Limit::new()),
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    thread_local! {
        pub(super) static LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn arti_warnings_are_printed_and_nothing_else() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid";
        let console = Console {
            limit: Mutex::new(Limit::new()),
        };
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
