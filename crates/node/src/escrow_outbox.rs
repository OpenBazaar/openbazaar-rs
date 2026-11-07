//! Escrow mail delivery: a persistent outbox behind a [`Transport`].
//!
//! [`Outbox::send`] stores a sealed letter in `escrow-outbox/` (sealed again
//! with the vault key, since the record names the peer's URL) before it
//! tries the peer once. A letter that does not get through is retried by
//! [`Outbox::flush`] with exponential backoff, from [`BACKOFF_BASE`] up to
//! [`BACKOFF_MAX`], and dropped after [`MAX_AGE`]. A peer that refuses the
//! letter outright (a 4xx) is not retried.
//!
//! Each letter is queued under an idempotency key: a hash of the recipient
//! URL and the plaintext message. Queuing the same message twice keeps one
//! entry, and a retry posts the identical sealed bytes, so the receiver's
//! replay guard ([`crate::letter_auth`]) drops a copy that already got
//! through before its response was lost.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use serde_json::{json, Value};

use crate::escrow_mail::{check_loopback, hex, unhex, Mail};
use crate::escrow_store::sync_dir;
use crate::vault::Vault;
use crate::Error;

pub(crate) const BACKOFF_BASE: Duration = Duration::from_secs(2);
pub(crate) const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);
pub(crate) const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const KEY_TAG: &[u8] = b"ob-node/escrow-outbox/v1";

/// Why a delivery did not happen.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Try again later: the peer was unreachable or failed.
    Retry(String),
    /// The peer refused the letter; retrying sends the same bytes again.
    Refused(String),
}

/// Moves one sealed letter to a peer's mail drop.
pub(crate) trait Transport: Send + Sync {
    fn deliver(&self, url: &str, letter: &[u8]) -> Result<(), Refusal>;
}

/// `POST {url}/escrow/mail` over loopback HTTP.
pub(crate) struct HttpTransport;

impl Transport for HttpTransport {
    fn deliver(&self, base_url: &str, letter: &[u8]) -> Result<(), Refusal> {
        check_loopback(base_url).map_err(Refusal::Refused)?;
        UrlTransport.deliver(&format!("{}/escrow/mail", base_url.trim_end_matches('/')), letter)
    }
}

/// `POST {url}` over loopback HTTP, the URL naming the route: order mail.
pub(crate) struct UrlTransport;

impl Transport for UrlTransport {
    fn deliver(&self, url: &str, letter: &[u8]) -> Result<(), Refusal> {
        check_loopback(url).map_err(Refusal::Refused)?;
        match ureq::post(url)
            .timeout(DELIVERY_TIMEOUT)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(letter)
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, _)) if (400..500).contains(&code) && code != 429 => {
                Err(Refusal::Refused(format!("{url}: {code}")))
            }
            Err(err) => Err(Refusal::Retry(format!("{url}: {err}"))),
        }
    }
}

/// What [`Outbox::send`] did with a letter.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    Delivered,
    /// Stored for retry, with the first failure.
    Queued(String),
}

pub(crate) struct Outbox {
    dir: PathBuf,
    vault: Vault,
    transport: Box<dyn Transport>,
    lock: Mutex<()>,
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn backoff(attempts: u32) -> u64 {
    BACKOFF_BASE
        .as_secs()
        .checked_shl(attempts.saturating_sub(1))
        .unwrap_or(u64::MAX)
        .min(BACKOFF_MAX.as_secs())
}

/// The idempotency key of `mail` sent to `url`.
pub(crate) fn mail_key(url: &str, mail: &Mail) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(KEY_TAG);
    engine.input(&(url.len() as u32).to_le_bytes());
    engine.input(url.trim_end_matches('/').as_bytes());
    engine.input(&mail.encode());
    sha256::Hash::from_engine(engine).to_byte_array()
}

impl Outbox {
    pub(crate) fn open(data_dir: &Path, vault: Vault, transport: Box<dyn Transport>) -> Result<Self, Error> {
        Self::open_named(data_dir, "escrow-outbox", vault, transport)
    }

    /// An outbox in `data_dir/name`.
    pub(crate) fn open_named(data_dir: &Path, name: &str, vault: Vault, transport: Box<dyn Transport>) -> Result<Self, Error> {
        let dir = data_dir.join(name);
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self {
            dir,
            vault,
            transport,
            lock: Mutex::new(()),
        })
    }

    fn path(&self, key: &[u8; 32]) -> PathBuf {
        self.dir.join(format!("{}.obv", hex(key)))
    }

    fn read(&self, path: &Path) -> Result<Option<Value>, String> {
        let bytes = self.vault.read_sealed(path).map_err(|err| err.to_string())?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|err| err.to_string()))
            .transpose()
    }

    fn write(&self, path: &Path, entry: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(entry).map_err(|err| err.to_string())?;
        self.vault.write_sealed(path, &bytes).map_err(|err| err.to_string())?;
        sync_dir(path)
    }

    fn remove(&self, path: &Path) {
        match fs::remove_file(path) {
            Ok(()) => {
                let _ = sync_dir(path);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => eprintln!("ob-node: escrow outbox {}: {err}", path.display()),
        }
    }

    /// Store `letter` for `url` under `key` for the next [`Outbox::flush`],
    /// without trying it: for a sender that must not wait on the peer.
    pub(crate) fn queue(&self, key: [u8; 32], url: &str, letter: &[u8]) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let path = self.path(&key);
        if self.read(&path)?.is_some() {
            return Ok(());
        }
        self.write(
            &path,
            &json!({ "url": url, "letter": hex(letter), "attempts": 0, "created": now_secs(), "nextAt": 0 }),
        )
    }

    /// Store `letter` for `url` under `key`, then try it once. An entry
    /// already queued under `key` is tried as it is, not replaced.
    pub(crate) fn send(&self, key: [u8; 32], url: &str, letter: &[u8]) -> Result<Sent, String> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let path = self.path(&key);
        let entry = match self.read(&path)? {
            Some(entry) => entry,
            None => {
                let entry = json!({
                    "url": url,
                    "letter": hex(letter),
                    "attempts": 0,
                    "created": now_secs(),
                    "nextAt": 0,
                });
                self.write(&path, &entry)?;
                entry
            }
        };
        match self.attempt(&path, entry, now_secs()) {
            Attempt::Delivered => Ok(Sent::Delivered),
            Attempt::Waiting(err) => Ok(Sent::Queued(err)),
            Attempt::Dropped(err) => Err(err),
        }
    }

    fn attempt(&self, path: &Path, mut entry: Value, now: u64) -> Attempt {
        let url = entry["url"].as_str().unwrap_or_default().to_string();
        let letter = match entry["letter"].as_str().map(unhex) {
            Some(Ok(letter)) => letter,
            _ => {
                self.remove(path);
                return Attempt::Dropped("a queued escrow letter is damaged".into());
            }
        };
        let error = match self.transport.deliver(&url, &letter) {
            Ok(()) => {
                self.remove(path);
                return Attempt::Delivered;
            }
            Err(Refusal::Refused(err)) => {
                self.remove(path);
                return Attempt::Dropped(err);
            }
            Err(Refusal::Retry(err)) => err,
        };
        let created = entry["created"].as_u64().unwrap_or(now);
        if now.saturating_sub(created) >= MAX_AGE.as_secs() {
            self.remove(path);
            return Attempt::Dropped(format!("gave up after {} hours: {error}", MAX_AGE.as_secs() / 3600));
        }
        let attempts = entry["attempts"].as_u64().unwrap_or(0) as u32 + 1;
        entry["attempts"] = json!(attempts);
        entry["nextAt"] = json!(now + backoff(attempts));
        entry["lastError"] = json!(error);
        if let Err(err) = self.write(path, &entry) {
            eprintln!("ob-node: escrow outbox: {err}");
        }
        Attempt::Waiting(error)
    }

    /// Retry every entry that is due at `now` (every entry with `force`).
    /// Returns how many were delivered.
    pub(crate) fn flush(&self, now: u64, force: bool) -> usize {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "obv"))
            .collect();
        paths.sort();
        let mut delivered = 0;
        for path in paths {
            let entry = match self.read(&path) {
                Ok(Some(entry)) => entry,
                Ok(None) => continue,
                Err(err) => {
                    eprintln!("ob-node: escrow outbox {}: {err}", path.display());
                    continue;
                }
            };
            if !force && entry["nextAt"].as_u64().unwrap_or(0) > now {
                continue;
            }
            match self.attempt(&path, entry, now) {
                Attempt::Delivered => delivered += 1,
                Attempt::Waiting(_) => {}
                Attempt::Dropped(err) => eprintln!("ob-node: escrow letter dropped: {err}"),
            }
        }
        delivered
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "obv"))
                    .count()
            })
            .unwrap_or(0)
    }
}

enum Attempt {
    Delivered,
    Waiting(String),
    Dropped(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow_mail::Kind;
    use crate::harness::TempDir;
    use std::sync::Arc;

    /// Fails the first `fail` deliveries, then records the rest.
    #[derive(Default)]
    pub(crate) struct Flaky {
        pub fail: Mutex<u32>,
        pub refuse: Mutex<bool>,
        pub got: Mutex<Vec<Vec<u8>>>,
    }

    impl Transport for Arc<Flaky> {
        fn deliver(&self, _: &str, letter: &[u8]) -> Result<(), Refusal> {
            if *self.refuse.lock().unwrap() {
                return Err(Refusal::Refused("400".into()));
            }
            let mut fail = self.fail.lock().unwrap();
            if *fail > 0 {
                *fail -= 1;
                return Err(Refusal::Retry("down".into()));
            }
            self.got.lock().unwrap().push(letter.to_vec());
            Ok(())
        }
    }

    fn outbox(dir: &Path, flaky: &Arc<Flaky>) -> Outbox {
        Outbox::open(dir, Vault::from_key(&[5; 32]), Box::new(flaky.clone())).unwrap()
    }

    #[test]
    fn a_failed_letter_is_retried_with_backoff_and_survives_a_restart() {
        let dir = TempDir::new();
        let flaky = Arc::new(Flaky::default());
        *flaky.fail.lock().unwrap() = 3;
        let mail = Mail::new(Kind::Request, [1; 16], vec![vec![1]]);
        let key = mail_key("http://127.0.0.1:1", &mail);
        let box1 = outbox(&dir.0, &flaky);
        assert!(matches!(box1.send(key, "http://127.0.0.1:1", b"sealed").unwrap(), Sent::Queued(_)));
        assert_eq!(box1.send(key, "http://127.0.0.1:1", b"other").unwrap(), Sent::Queued("down".into()));
        assert_eq!(box1.pending(), 1, "one entry per idempotency key");

        let now = now_secs();
        assert_eq!(box1.flush(now, false), 0, "not due yet");
        drop(box1);
        let box2 = outbox(&dir.0, &flaky);
        assert_eq!(box2.flush(now + BACKOFF_MAX.as_secs(), false), 0, "third failure");
        assert_eq!(box2.flush(now + 2 * BACKOFF_MAX.as_secs(), false), 1);
        assert_eq!(box2.pending(), 0);
        assert_eq!(*flaky.got.lock().unwrap(), vec![b"sealed".to_vec()], "the first sealed bytes, once");
        assert_eq!(backoff(1), 2);
        assert_eq!(backoff(4), 16);
        assert_eq!(backoff(40), BACKOFF_MAX.as_secs());
    }

    #[test]
    fn a_refused_or_expired_letter_is_dropped() {
        let dir = TempDir::new();
        let flaky = Arc::new(Flaky::default());
        *flaky.refuse.lock().unwrap() = true;
        let outbox = outbox(&dir.0, &flaky);
        assert!(outbox.send([2; 32], "http://127.0.0.1:1", b"x").is_err());
        assert_eq!(outbox.pending(), 0);

        *flaky.refuse.lock().unwrap() = false;
        *flaky.fail.lock().unwrap() = u32::MAX;
        assert!(matches!(outbox.send([3; 32], "http://127.0.0.1:1", b"y").unwrap(), Sent::Queued(_)));
        assert_eq!(outbox.flush(now_secs() + MAX_AGE.as_secs(), true), 0);
        assert_eq!(outbox.pending(), 0, "dropped after the maximum age");
    }

    #[test]
    fn http_transport_refuses_non_loopback_peers_without_dialing() {
        assert!(matches!(HttpTransport.deliver("http://10.0.0.1:1", b"x"), Err(Refusal::Refused(_))));
        assert!(matches!(HttpTransport.deliver("http://127.0.0.1:9", b"x"), Err(Refusal::Retry(_))));
    }
}
