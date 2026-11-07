//! One-time pairing codes.
//!
//! A code is `obhome:{secret}@{address}`: 160 random bits in lowercase
//! base32, and the address a device dials (the node's onion, or a loopback
//! address in tests). It is shown on the node's console or on an already
//! paired device, never sent by the node to anyone who does not hold one.
//! A code pairs one device, lives [`OFFER_LIFETIME`], and at most
//! [`MAX_LIVE`] are open at once. Wrong secrets are counted node-wide, and
//! past [`MAX_FAILURES`] in [`FAILURE_WINDOW`] pairing pauses.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::http_guard::secret_eq;
use crate::vault_key::random_bytes;

pub(crate) const PREFIX: &str = "obhome:";
pub(crate) const OFFER_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_LIVE: usize = 4;
const MAX_FAILURES: usize = 20;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);

struct Offer {
    secret: String,
    created: Instant,
    by: Option<String>,
}

#[derive(Default)]
pub(crate) struct Offers {
    live: Vec<Offer>,
    failures: VecDeque<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// The device that made the code, or `None` for the node itself.
    Paired(Option<String>),
    Wrong,
    Paused,
}

impl Offers {
    fn prune(&mut self, now: Instant) {
        self.live.retain(|offer| now.duration_since(offer.created) < OFFER_LIFETIME);
        while self.failures.front().is_some_and(|at| now.duration_since(*at) >= FAILURE_WINDOW) {
            self.failures.pop_front();
        }
    }

    /// A new secret. The oldest open code closes when [`MAX_LIVE`] are open.
    pub(crate) fn issue(&mut self, by: Option<String>, now: Instant) -> String {
        self.prune(now);
        if self.live.len() >= MAX_LIVE {
            self.live.remove(0);
        }
        let secret = base32(&random_bytes::<20>());
        self.live.push(Offer { secret: secret.clone(), created: now, by });
        secret
    }

    pub(crate) fn take(&mut self, presented: &str, now: Instant) -> Taken {
        self.prune(now);
        if self.failures.len() >= MAX_FAILURES {
            return Taken::Paused;
        }
        let presented = presented.trim().to_ascii_lowercase();
        match self.live.iter().position(|offer| secret_eq(Some(&presented), &offer.secret)) {
            Some(index) => Taken::Paired(self.live.remove(index).by),
            None => {
                self.failures.push_back(now);
                Taken::Wrong
            }
        }
    }

    /// Close every code a revoked device made.
    pub(crate) fn drop_from(&mut self, devices: &[String]) {
        self.live.retain(|offer| !offer.by.as_ref().is_some_and(|by| devices.contains(by)));
    }
}

pub(crate) fn code(secret: &str, address: &str) -> String {
    format!("{PREFIX}{secret}@{address}")
}

/// `(secret, address)` from a code a user pasted.
pub(crate) fn parse_code(text: &str) -> Option<(String, String)> {
    let rest = text.trim().strip_prefix(PREFIX)?;
    let (secret, address) = rest.split_once('@')?;
    let secret = secret.to_ascii_lowercase();
    let ok = secret.len() == 32 && secret.bytes().all(|b| b"abcdefghijklmnopqrstuvwxyz234567".contains(&b));
    (ok && !address.is_empty()).then(|| (secret, address.to_ascii_lowercase()))
}

fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_pairs_once_and_expires() {
        let mut offers = Offers::default();
        let now = Instant::now();
        let secret = offers.issue(None, now);
        assert_eq!(secret.len(), 32);
        let (parsed, address) = parse_code(&code(&secret.to_uppercase(), "Example.onion")).unwrap();
        assert_eq!((parsed.as_str(), address.as_str()), (secret.as_str(), "example.onion"));
        assert_eq!(offers.take(&secret, now), Taken::Paired(None));
        assert_eq!(offers.take(&secret, now), Taken::Wrong, "one use");
        let late = offers.issue(Some("aa".into()), now);
        assert_eq!(offers.take(&late, now + OFFER_LIFETIME), Taken::Wrong);
    }

    #[test]
    fn guessing_pauses_pairing() {
        let mut offers = Offers::default();
        let now = Instant::now();
        let secret = offers.issue(None, now);
        for _ in 0..MAX_FAILURES {
            assert_eq!(offers.take("a".repeat(32).as_str(), now), Taken::Wrong);
        }
        assert_eq!(offers.take(&secret, now), Taken::Paused);
        assert_eq!(offers.take(&secret, now + FAILURE_WINDOW), Taken::Paired(None));
    }

    #[test]
    fn codes_from_a_revoked_device_close() {
        let mut offers = Offers::default();
        let now = Instant::now();
        let secret = offers.issue(Some("dev".into()), now);
        offers.drop_from(&["dev".into()]);
        assert_eq!(offers.take(&secret, now), Taken::Wrong);
    }

    #[test]
    fn malformed_codes_are_refused() {
        for text in ["", "obhome:", "obhome:abc@x.onion", "home:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@x.onion", "obhome:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@", "obhome:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1@x"] {
            assert!(parse_code(text).is_none(), "{text}");
        }
    }
}
