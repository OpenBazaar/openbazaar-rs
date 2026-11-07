//! Durable escrow state: one sealed, versioned record per escrow and role.
//!
//! Records live in `escrow/{buyer,vendor,moderator}/` in the data dir. Each
//! is a JSON object sealed with the vault key (the receive tier), so the
//! node can read them while the wallet is locked. No record holds a signing
//! secret: the escrow crate's snapshots carry no secret key and no secret
//! nonce, and restoring one takes the per-order key, which comes from the
//! seed and so only from the spend tier ([`crate::wallet_lock`]).
//!
//! A write goes to a temporary file, is synced, renamed over the record,
//! and the directory is synced, so a crash leaves the old record or the new
//! one. Every record carries [`RECORD_VERSION`]; a newer one is refused, not
//! guessed at.
//!
//! [`OrderLocks`] serializes the steps of one escrow without holding up any
//! other escrow.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::escrow_mail::{id_hex, EscrowId};
use crate::vault::Vault;
use crate::Error;

pub(crate) const RECORD_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Buyer,
    Vendor,
    Moderator,
}

impl Role {
    fn dir(self) -> &'static str {
        match self {
            Role::Buyer => "buyer",
            Role::Vendor => "vendor",
            Role::Moderator => "moderator",
        }
    }
}

pub(crate) struct EscrowStore {
    dir: PathBuf,
    vault: Vault,
}

impl EscrowStore {
    pub(crate) fn open(data_dir: &Path, vault: Vault) -> Result<Self, Error> {
        let dir = data_dir.join("escrow");
        for role in [Role::Buyer, Role::Vendor, Role::Moderator] {
            fs::create_dir_all(dir.join(role.dir())).map_err(|err| Error::Detail(err.to_string()))?;
        }
        Ok(Self { dir, vault })
    }

    fn path(&self, role: Role, name: &str) -> PathBuf {
        self.dir.join(role.dir()).join(format!("{name}.obv"))
    }

    pub(crate) fn save(&self, role: Role, name: &str, record: &Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(&json!({ "v": RECORD_VERSION, "record": record }))
            .map_err(|err| err.to_string())?;
        let path = self.path(role, name);
        self.vault.write_sealed(&path, &bytes).map_err(|err| err.to_string())?;
        sync_dir(&path)
    }

    pub(crate) fn load(&self, role: Role, name: &str) -> Result<Option<Value>, String> {
        let Some(bytes) = self
            .vault
            .read_sealed(&self.path(role, name))
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
        match value.get("v").and_then(Value::as_u64) {
            Some(RECORD_VERSION) => Ok(value.get("record").cloned()),
            Some(newer) => Err(format!("escrow record {name} is version {newer}; this node reads {RECORD_VERSION}")),
            None => Err(format!("escrow record {name} has no version")),
        }
    }

    /// Every record of `role`, by name. A record that cannot be read is
    /// logged and skipped, so one damaged file does not hide the rest.
    pub(crate) fn list(&self, role: Role) -> Vec<(String, Value)> {
        let Ok(entries) = fs::read_dir(self.dir.join(role.dir())) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".obv").map(str::to_string))
            .collect();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| match self.load(role, &name) {
                Ok(Some(record)) => Some((name, record)),
                Ok(None) => None,
                Err(err) => {
                    eprintln!("ob-node: escrow record {name}: {err}");
                    None
                }
            })
            .collect()
    }
}

/// Sync the directory holding `path`, so a rename into it survives a crash.
pub(crate) fn sync_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        fs::File::open(dir).and_then(|dir| dir.sync_all()).map_err(|err| err.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// One lock per escrow. Hold the returned lock for a whole step of that
/// escrow; steps of other escrows run in parallel.
#[derive(Default)]
pub(crate) struct OrderLocks(Mutex<HashMap<EscrowId, Arc<Mutex<()>>>>);

impl OrderLocks {
    pub(crate) fn get(&self, id: &EscrowId) -> Arc<Mutex<()>> {
        let mut locks = self.0.lock().unwrap_or_else(|err| err.into_inner());
        if locks.len() > 4096 {
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        locks.entry(*id).or_default().clone()
    }
}

/// The record name of an escrow id.
pub(crate) fn name(id: &EscrowId) -> String {
    id_hex(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;

    #[test]
    fn records_round_trip_sealed_and_refuse_a_newer_version() {
        let dir = TempDir::new();
        let vault = Vault::from_key(&[7; 32]);
        let store = EscrowStore::open(&dir.0, vault.clone()).unwrap();
        store.save(Role::Vendor, "ab", &json!({"stage": "offered", "secretish": "c0ffee"})).unwrap();
        assert_eq!(store.load(Role::Vendor, "ab").unwrap().unwrap()["stage"], "offered");
        assert!(store.load(Role::Buyer, "ab").unwrap().is_none());
        assert_eq!(store.list(Role::Vendor).len(), 1);

        let raw = fs::read(dir.0.join("escrow/vendor/ab.obv")).unwrap();
        assert!(!raw.windows(7).any(|w| w == b"offered"), "records are sealed");
        let other = EscrowStore::open(&dir.0, Vault::from_key(&[8; 32])).unwrap();
        assert!(other.load(Role::Vendor, "ab").is_err(), "another vault key cannot open it");

        let newer = serde_json::to_vec(&json!({"v": RECORD_VERSION + 1, "record": {}})).unwrap();
        vault.write_sealed(&dir.0.join("escrow/vendor/cd.obv"), &newer).unwrap();
        assert!(store.load(Role::Vendor, "cd").unwrap_err().contains("version"));
        assert_eq!(store.list(Role::Vendor).len(), 1, "an unreadable record is skipped");
    }

    #[test]
    fn order_locks_serialize_one_escrow_and_not_another() {
        let locks = OrderLocks::default();
        let a = locks.get(&[1; 16]);
        let b = locks.get(&[2; 16]);
        let _held = a.lock().unwrap();
        assert!(locks.get(&[1; 16]).try_lock().is_err());
        assert!(b.try_lock().is_ok());
    }
}
