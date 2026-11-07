//! The moderator's durable ruling store: at most one branch per escrow,
//! across restarts and crashes.
//!
//! Each ruling is one file in `rulings/`, named by a tagged hash of the
//! [`RulingKey`] (so the directory listing names no outpoint) and sealed
//! with the vault key. [`FileRulingStore::record_once`] writes the record to
//! a temporary file, syncs it, and then hard-links it to its final name. The
//! link either creates the complete file or fails because the name exists,
//! so two callers, or a caller and a crash, can never leave two branches or
//! a half-written one. The directory is synced before the call returns, and
//! [`openbazaar_escrow::Moderator::open`] records the branch before it
//! decrypts anything.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use openbazaar_escrow::{Error as EscrowError, Party, RulingKey, RulingStore};
use serde_json::{json, Value};

use crate::escrow_store::sync_dir;
use crate::vault::Vault;
use crate::vault_key::{encode_hex, random_bytes};
use crate::Error;

const NAME_TAG: &[u8] = b"ob-node/ruling-name/v1";

pub(crate) struct FileRulingStore {
    dir: PathBuf,
    vault: Vault,
}

impl FileRulingStore {
    pub(crate) fn open(data_dir: &Path, vault: Vault) -> Result<Self, Error> {
        let dir = data_dir.join("rulings");
        fs::create_dir_all(&dir).map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self { dir, vault })
    }

    fn path(&self, key: &RulingKey) -> PathBuf {
        let mut engine = sha256::Hash::engine();
        engine.input(NAME_TAG);
        engine.input(&key.terms_digest);
        engine.input(key.outpoint.txid.as_byte_array());
        engine.input(&key.outpoint.vout.to_le_bytes());
        self.dir.join(format!("{}.ruling", sha256::Hash::from_engine(engine)))
    }

    /// The branch recorded for `key`, if any.
    pub(crate) fn get(&self, key: &RulingKey) -> Result<Option<Party>, String> {
        let bytes = match fs::read(self.path(key)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        let value: Value = serde_json::from_slice(&self.vault.open_bytes(&bytes).map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
        match value.get("favored").and_then(Value::as_str) {
            Some("buyer") => Ok(Some(Party::Buyer)),
            Some("vendor") => Ok(Some(Party::Vendor)),
            _ => Err("a ruling record names no branch".into()),
        }
    }

    fn record(&self, key: &RulingKey, favored: Party) -> Result<Party, String> {
        if let Some(recorded) = self.get(key)? {
            return Ok(recorded);
        }
        let final_path = self.path(key);
        let tmp = self.dir.join(format!("{}.tmp", encode_hex(&random_bytes::<16>())));
        let body = json!({
            "favored": match favored { Party::Buyer => "buyer", Party::Vendor => "vendor" },
            "termsDigest": encode_hex(&key.terms_digest),
            "outpoint": key.outpoint.to_string(),
        });
        let sealed = self
            .vault
            .seal(&serde_json::to_vec(&body).map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
        crate::vault::write_secret(&tmp, &sealed).map_err(|err| err.to_string())?;
        let linked = fs::hard_link(&tmp, &final_path);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => {
                sync_dir(&final_path)?;
                Ok(favored)
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                self.get(key)?.ok_or_else(|| "a ruling record vanished".to_string())
            }
            Err(err) => Err(err.to_string()),
        }
    }
}

impl RulingStore for &FileRulingStore {
    fn record_once(&mut self, key: RulingKey, favored: Party) -> Result<Party, EscrowError> {
        self.record(&key, favored).map_err(|err| {
            eprintln!("ob-node: ruling store: {err}");
            EscrowError::RulingStore("the ruling could not be recorded")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use bitcoin::OutPoint;
    use std::sync::{Arc, Barrier};

    fn key(n: u8) -> RulingKey {
        RulingKey {
            terms_digest: [n; 32],
            outpoint: OutPoint::null(),
        }
    }

    #[test]
    fn one_branch_per_escrow_survives_a_reopen_and_leftover_temp_files() {
        let dir = TempDir::new();
        let vault = Vault::from_key(&[3; 32]);
        let store = FileRulingStore::open(&dir.0, vault.clone()).unwrap();
        assert_eq!((&store).record_once(key(1), Party::Buyer).unwrap(), Party::Buyer);
        assert_eq!((&store).record_once(key(1), Party::Vendor).unwrap(), Party::Buyer);
        assert_eq!((&store).record_once(key(2), Party::Vendor).unwrap(), Party::Vendor);

        // A crash between the temp write and the link leaves only a temp file.
        fs::write(dir.0.join("rulings/dead.tmp"), b"half").unwrap();
        drop(store);
        let store = FileRulingStore::open(&dir.0, vault).unwrap();
        assert_eq!(store.get(&key(1)).unwrap(), Some(Party::Buyer));
        assert_eq!((&store).record_once(key(1), Party::Vendor).unwrap(), Party::Buyer);
        assert_eq!(store.get(&key(3)).unwrap(), None);
        let names: Vec<String> = fs::read_dir(dir.0.join("rulings"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|name| !name.contains(&"01".repeat(32))), "names do not leak the key");
    }

    #[test]
    fn racing_rulings_for_both_branches_record_exactly_one() {
        for round in 0..8u8 {
            let dir = TempDir::new();
            let store = Arc::new(FileRulingStore::open(&dir.0, Vault::from_key(&[4; 32])).unwrap());
            let barrier = Arc::new(Barrier::new(8));
            let threads: Vec<_> = (0..8)
                .map(|n| {
                    let (store, barrier) = (store.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        let favored = if n % 2 == 0 { Party::Buyer } else { Party::Vendor };
                        barrier.wait();
                        (&*store).record_once(key(round), favored).unwrap()
                    })
                })
                .collect();
            let results: Vec<Party> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert!(results.windows(2).all(|pair| pair[0] == pair[1]), "{results:?}");
            assert_eq!(store.get(&key(round)).unwrap(), Some(results[0]));
        }
    }
}
