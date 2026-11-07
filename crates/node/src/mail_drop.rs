//! Limits on `POST /escrow/mail`, the one route peers call without a token.
//!
//! The route only queues bytes that parse as one sealed letter; the mail loop
//! opens them later. Before that, this module refuses: any request from a
//! browser page (peers never send `Origin`), a body over the sealed-letter
//! maximum, more than [`BURST`] letters at once or [`PER_SECOND`] sustained,
//! and any letter while [`MAX_PENDING`] are already waiting.

use std::sync::Mutex;
use std::time::Instant;

use crate::http::json_error;
use crate::http_parse::has_header;
use crate::Node;

/// tormail's `MAX_SEALED_LEN`, which the crate does not export.
const MAX_SEALED_LEN: usize = 64 * 1024;
const BURST: f64 = 64.0;
const PER_SECOND: f64 = 4.0;
pub(crate) const MAX_PENDING: usize = 256;

pub(crate) struct MailDrop {
    bucket: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

impl MailDrop {
    pub(crate) fn new() -> Self {
        Self {
            bucket: Mutex::new(Bucket {
                tokens: BURST,
                at: Instant::now(),
            }),
        }
    }

    fn take(&self, now: Instant) -> bool {
        let mut bucket = self.bucket.lock().unwrap_or_else(|err| err.into_inner());
        let elapsed = now.saturating_duration_since(bucket.at).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * PER_SECOND).min(BURST);
        bucket.at = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }
}

/// `Err` carries the refusal to send instead of queueing the letter.
pub(crate) fn admit(node: &Node, head: &str, body: &[u8]) -> Result<(), Vec<u8>> {
    admit_mail(node, head, body)?;
    let pending = node.escrow.inbox.list().map(|ids| ids.len()).unwrap_or(MAX_PENDING);
    if pending >= MAX_PENDING {
        return Err(json_error("503 Service Unavailable", "The escrow inbox is full.", None));
    }
    Ok(())
}

/// The limits every peer mail route shares: order mail (`POST /mail`,
/// `POST /mail/reply`) is handled as it arrives, so it has no inbox to fill.
pub(crate) fn admit_mail(node: &Node, head: &str, body: &[u8]) -> Result<(), Vec<u8>> {
    if has_header(head, "origin") {
        return Err(json_error("403 Forbidden", "Mail comes from nodes, not pages.", None));
    }
    if body.len() > MAX_SEALED_LEN {
        return Err(json_error("413 Content Too Large", "That letter is too large.", None));
    }
    if !node.mail_drop.take(Instant::now()) {
        return Err(json_error("429 Too Many Requests", "Too many letters. Try again shortly.", None));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_bucket_refills_at_the_sustained_rate() {
        let drop = MailDrop::new();
        let start = Instant::now();
        for _ in 0..BURST as usize {
            assert!(drop.take(start));
        }
        assert!(!drop.take(start));
        assert!(drop.take(start + Duration::from_secs_f64(1.0 / PER_SECOND)));
        assert!(!drop.take(start + Duration::from_secs_f64(1.0 / PER_SECOND)));
    }
}
