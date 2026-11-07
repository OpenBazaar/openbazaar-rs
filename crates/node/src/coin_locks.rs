//! Wallet coins reserved for an escrow funding that is not signed yet.
//!
//! Round 1 ([`openbazaar_escrow::BuyerSession::start`]) fixes the funding
//! PSBT, and so its txid and the escrow outpoint, before the vendor replies.
//! Until the funding is signed and broadcast, the wallet still counts those
//! inputs as spendable. A second payment that spent one of them would make
//! the escrow outpoint unfundable after both parties signed for it. So the
//! inputs are locked here from `BuyerOpen` on, and every payment the node
//! builds skips them.
//!
//! A lock taken at `BuyerOpen` expires after [`OPEN_LOCK_TTL`]; the watcher
//! then aborts the open and releases it. Signing the funding (the buyer's
//! `BuyerSignatures`) makes the lock permanent until the funding is
//! broadcast, and a failed round releases it at once. Locks are sealed in
//! `coin-locks.obv`, so a restart keeps them.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use bitcoin::OutPoint;
use serde_json::{json, Value};

use crate::vault::Vault;
use crate::Error;

pub(crate) const OPEN_LOCK_TTL: Duration = crate::escrow_outbox::MAX_AGE;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lock {
    outpoint: OutPoint,
    order: String,
    /// Unix seconds; `None` holds until released.
    expires: Option<u64>,
}

pub(crate) struct CoinLocks {
    path: PathBuf,
    vault: Vault,
    locks: Mutex<Vec<Lock>>,
}

impl CoinLocks {
    pub(crate) fn open(data_dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = data_dir.join("coin-locks.obv");
        let mut locks = Vec::new();
        if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|err| Error::Detail(err.to_string()))?;
            for item in value.as_array().into_iter().flatten() {
                let outpoint = item["outpoint"].as_str().and_then(|text| text.parse().ok());
                let order = item["order"].as_str();
                if let (Some(outpoint), Some(order)) = (outpoint, order) {
                    locks.push(Lock {
                        outpoint,
                        order: order.to_string(),
                        expires: item["expires"].as_u64(),
                    });
                }
            }
        }
        Ok(Self {
            path,
            vault,
            locks: Mutex::new(locks),
        })
    }

    fn save(&self, locks: &[Lock]) -> Result<(), String> {
        let value: Vec<Value> = locks
            .iter()
            .map(|lock| json!({"outpoint": lock.outpoint.to_string(), "order": lock.order, "expires": lock.expires}))
            .collect();
        let bytes = serde_json::to_vec(&value).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())?;
        crate::escrow_store::sync_dir(&self.path)
    }

    fn update(&self, change: impl FnOnce(&mut Vec<Lock>)) -> Result<(), String> {
        let mut locks = self.locks.lock().unwrap_or_else(|err| err.into_inner());
        let mut next = locks.clone();
        change(&mut next);
        if next != *locks {
            self.save(&next)?;
            *locks = next;
        }
        Ok(())
    }

    /// Lock `outpoints` for `order`, replacing what it held before.
    pub(crate) fn lock(&self, order: &str, outpoints: &[OutPoint], expires: Option<u64>) -> Result<(), String> {
        self.update(|locks| {
            locks.retain(|lock| lock.order != order);
            locks.extend(outpoints.iter().map(|outpoint| Lock {
                outpoint: *outpoint,
                order: order.to_string(),
                expires,
            }));
        })
    }

    /// Keep `order`'s coins locked until [`Self::release`].
    pub(crate) fn hold(&self, order: &str) -> Result<(), String> {
        self.update(|locks| {
            for lock in locks.iter_mut().filter(|lock| lock.order == order) {
                lock.expires = None;
            }
        })
    }

    pub(crate) fn release(&self, order: &str) -> Result<(), String> {
        self.update(|locks| locks.retain(|lock| lock.order != order))
    }

    /// Every locked coin. Payments must not spend these.
    pub(crate) fn outpoints(&self) -> Vec<OutPoint> {
        self.locks.lock().unwrap_or_else(|err| err.into_inner()).iter().map(|lock| lock.outpoint).collect()
    }

    /// Orders with a lock that expired at `now`. They stay locked until released.
    pub(crate) fn expired(&self, now: u64) -> Vec<String> {
        let locks = self.locks.lock().unwrap_or_else(|err| err.into_inner());
        let mut orders: Vec<String> = locks
            .iter()
            .filter(|lock| lock.expires.is_some_and(|at| at <= now))
            .map(|lock| lock.order.clone())
            .collect();
        orders.dedup();
        orders
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use bitcoin::hashes::Hash;

    fn coin(n: u8) -> OutPoint {
        OutPoint {
            txid: bitcoin::Txid::from_byte_array([n; 32]),
            vout: 0,
        }
    }

    #[test]
    fn locks_persist_expire_and_hold() {
        let dir = TempDir::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        let vault = Vault::from_key(&[9; 32]);
        let locks = CoinLocks::open(&dir.0, vault.clone()).unwrap();
        locks.lock("a", &[coin(1), coin(2)], Some(100)).unwrap();
        locks.lock("b", &[coin(3)], Some(50)).unwrap();
        assert_eq!(locks.expired(60), vec!["b".to_string()]);
        locks.hold("a").unwrap();
        assert!(locks.expired(1_000).iter().all(|order| order == "b"));

        let locks = CoinLocks::open(&dir.0, vault).unwrap();
        assert_eq!(locks.outpoints().len(), 3);
        locks.release("b").unwrap();
        locks.lock("a", &[coin(4)], None).unwrap();
        assert_eq!(locks.outpoints(), vec![coin(4)]);
    }
}
