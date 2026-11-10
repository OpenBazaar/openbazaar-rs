//! Delivery of a sealed order to the onion in the vendor's signed store record.
//!
//! Checkout keeps the buyer's copy in the local mailbox and queues the same
//! bytes here. The queue does not dial. [`OrderOutbox::flush`] dials, so an
//! HTTP request is not stuck on Tor. A letter that does not get through is
//! retried with the escrow outbox's backoff and dropped after the same day.
//!
//! The Arti state for dialing lives in `tor-client/`, not `tor/`. The
//! listener holds the only client on `tor/`, and a second client there would
//! fight it for the onion identity.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::{now_secs, BACKOFF_BASE, BACKOFF_MAX, MAX_AGE};
use crate::escrow_store::sync_dir;
use crate::vault::Vault;
use crate::{Error, Node};

const DIAL_TIMEOUT: Duration = Duration::from_secs(120);
const KEY_TAG: &[u8] = b"ob-node/order-outbox/v1";

trait LetterTransport: Send + Sync {
    fn deliver(&self, onion: &str, letter: &[u8]) -> Result<(), String>;
}

struct ArtiTransport {
    state_dir: PathBuf,
}

impl LetterTransport for ArtiTransport {
    fn deliver(&self, onion: &str, letter: &[u8]) -> Result<(), String> {
        crate::onion_mail::deliver_letter(&self.state_dir, onion, letter, DIAL_TIMEOUT)
    }
}

pub(crate) struct OrderOutbox {
    dir: PathBuf,
    vault: Vault,
    transport: Box<dyn LetterTransport>,
    lock: Mutex<()>,
}

impl OrderOutbox {
    pub(crate) fn open(data_dir: &Path, vault: Vault) -> Result<Self, Error> {
        Self::with_transport(
            data_dir,
            vault,
            Box::new(ArtiTransport { state_dir: data_dir.join("tor-client") }),
        )
    }

    fn with_transport(data_dir: &Path, vault: Vault, transport: Box<dyn LetterTransport>) -> Result<Self, Error> {
        let dir = data_dir.join("order-outbox");
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self { dir, vault, transport, lock: Mutex::new(()) })
    }

    /// Remember `letter` for `onion`. Does not dial. The same letter queued
    /// twice stays one entry.
    pub(crate) fn enqueue(&self, onion: &str, letter: &[u8]) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|err| err.into_inner());
        let path = self.path(&letter_key(onion, letter));
        if self.read(&path)?.is_some() {
            return Ok(());
        }
        let entry = json!({
            "onion": onion,
            "letter": hex(letter),
            "attempts": 0,
            "created": now_secs(),
            "nextAt": 0,
        });
        self.write(&path, &entry)
    }

    /// Dial every letter that is due at `now`. Returns how many were stored
    /// by the vendor.
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
                    eprintln!("ob-node: order outbox {}: {err}", path.display());
                    continue;
                }
            };
            if !force && entry["nextAt"].as_u64().unwrap_or(0) > now {
                continue;
            }
            if self.attempt(&path, entry, now) {
                delivered += 1;
            }
        }
        delivered
    }

    fn attempt(&self, path: &Path, mut entry: Value, now: u64) -> bool {
        let onion = entry["onion"].as_str().unwrap_or_default().to_string();
        let letter = match entry["letter"].as_str().map(unhex) {
            Some(Ok(letter)) => letter,
            _ => {
                self.remove(path);
                eprintln!("ob-node: order letter dropped: a queued letter is damaged");
                return false;
            }
        };
        if let Err(err) = self.transport.deliver(&onion, &letter) {
            let created = entry["created"].as_u64().unwrap_or(now);
            if now.saturating_sub(created) >= MAX_AGE.as_secs() {
                self.remove(path);
                eprintln!("ob-node: order letter dropped: gave up after a day: {err}");
                return false;
            }
            let attempts = entry["attempts"].as_u64().unwrap_or(0) as u32 + 1;
            entry["attempts"] = json!(attempts);
            entry["nextAt"] = json!(now + backoff(attempts));
            entry["lastError"] = json!(err);
            if let Err(err) = self.write(path, &entry) {
                eprintln!("ob-node: order outbox: {err}");
            }
            return false;
        }
        self.remove(path);
        true
    }

    fn path(&self, key: &[u8; 32]) -> PathBuf {
        self.dir.join(format!("{}.obv", hex(key)))
    }

    fn read(&self, path: &Path) -> Result<Option<Value>, String> {
        let bytes = self.vault.read_sealed(path).map_err(|err| err.to_string())?;
        bytes.map(|bytes| serde_json::from_slice(&bytes).map_err(|err| err.to_string())).transpose()
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
            Err(err) => eprintln!("ob-node: order outbox {}: {err}", path.display()),
        }
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        fs::read_dir(&self.dir)
            .map(|entries| {
                entries.flatten().filter(|entry| entry.path().extension().is_some_and(|ext| ext == "obv")).count()
            })
            .unwrap_or(0)
    }
}

fn letter_key(onion: &str, letter: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(KEY_TAG);
    engine.input(onion.as_bytes());
    engine.input(letter);
    sha256::Hash::from_engine(engine).to_byte_array()
}

fn backoff(attempts: u32) -> u64 {
    BACKOFF_BASE.as_secs().checked_shl(attempts.saturating_sub(1)).unwrap_or(u64::MAX).min(BACKOFF_MAX.as_secs())
}

impl Node {
    /// Retry queued order letters until `stop` is set. Dialing happens here,
    /// off the HTTP thread.
    pub(crate) fn run_order_outbox(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            self.order_outbox.flush(now_secs(), false);
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use crate::vault::Vault;
    use std::sync::Arc;

    struct Recording {
        fail: Mutex<u32>,
        got: Mutex<Vec<Vec<u8>>>,
    }

    impl LetterTransport for Arc<Recording> {
        fn deliver(&self, _: &str, letter: &[u8]) -> Result<(), String> {
            let mut fail = self.fail.lock().unwrap();
            if *fail > 0 {
                *fail -= 1;
                return Err("down".into());
            }
            self.got.lock().unwrap().push(letter.to_vec());
            Ok(())
        }
    }

    #[test]
    fn a_queued_letter_is_dialed_later_and_not_replaced() {
        let dir = TempDir::new();
        let recording = Arc::new(Recording { fail: Mutex::new(1), got: Mutex::new(Vec::new()) });
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";
        let outbox = OrderOutbox::with_transport(&dir.0, Vault::from_key(&[7; 32]), Box::new(recording.clone())).unwrap();
        outbox.enqueue(onion, b"sealed").unwrap();
        outbox.enqueue(onion, b"sealed").unwrap();
        assert_eq!(outbox.pending(), 1, "one entry for one letter");
        let now = now_secs();
        assert_eq!(outbox.flush(now, true), 0, "the first dial fails");
        assert_eq!(outbox.flush(now, false), 0, "the retry waits for backoff");
        assert_eq!(outbox.flush(now + backoff(1), false), 1);
        assert_eq!(outbox.pending(), 0);
        assert_eq!(recording.got.lock().unwrap().as_slice(), [b"sealed".to_vec()]);
    }
}
