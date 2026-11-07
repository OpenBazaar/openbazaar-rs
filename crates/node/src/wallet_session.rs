//! The node's side of the wallet lock: setup, unlock, and access to the spend tier.
//!
//! Setup and unlock move any vault-tier `seed.obv` into the spend tier.
//! Setup writes every `spend.obv` first, then `wallet.lock` (the commit
//! point), and only then deletes the `seed.obv` files, so a crash at any
//! step leaves a dir that opens with the old seed or the new lock.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bitcoin::secp256k1::SecretKey;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::pay_order::PayError;
use crate::seed::Seed;
use crate::wallet::NodeWallet;
use crate::wallet_keys;
use crate::wallet_lock::{LockError, State};
use crate::{Node, OrderError};

const AUTO_LOCK_POLL: Duration = Duration::from_secs(1);

impl From<LockError> for PayError {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Locked(state) => PayError::Locked(state.as_str()),
            LockError::Wrong { .. } | LockError::Backoff { .. } | LockError::Invalid(_) => {
                PayError::Invalid("The wallet could not confirm this step.".into())
            }
            LockError::Failed(detail) => PayError::Failed(detail),
        }
    }
}

impl Node {
    /// Every wallet's directory: the node wallet first, then each store wallet.
    fn wallet_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.buyer_wallet().dir.clone()];
        for wallet in self.store_wallets.values() {
            dirs.push(wallet.lock().unwrap_or_else(|err| err.into_inner()).dir.clone());
        }
        dirs
    }

    /// A data dir that never finished setup may hold `spend.obv` files for a
    /// key that was never written down. Drop them; `seed.obv` is still there.
    pub(crate) fn drop_uncommitted_spend(&self) -> Result<(), crate::Error> {
        if self.lock.state() != State::Setup {
            return Ok(());
        }
        for dir in self.wallet_dirs() {
            wallet_keys::drop_uncommitted(&dir)?;
        }
        Ok(())
    }

    /// The seed of `wallet`, for one signature. Refused while locked.
    pub(crate) fn spend_seed(&self, wallet: &NodeWallet) -> Result<Seed, LockError> {
        let key = self.lock.spend_key()?;
        wallet_keys::spend_seed(&wallet.dir, &key, self.network).map_err(LockError::from)
    }

    /// The per-order escrow key `index` of `wallet`. Refused while locked.
    pub(crate) fn escrow_secret(&self, wallet: &NodeWallet, index: u32) -> Result<SecretKey, String> {
        self.spend_seed(wallet)
            .map(|seed| seed.escrow_key(index))
            .map_err(|err| match err {
                LockError::Locked(_) => "the wallet is locked".to_string(),
                other => format!("{other:?}"),
            })
    }

    /// 423 for a checkout that would derive a signing key while locked.
    pub(crate) fn require_unlocked(&self) -> Result<(), OrderError> {
        match self.lock.state() {
            State::Unlocked => Ok(()),
            state => Err(OrderError::Locked(state.as_str())),
        }
    }

    /// `POST /setup`: set the first passphrase, move every seed into the
    /// spend tier, and return the node wallet's phrase this one time.
    pub(crate) fn setup_wallet(&self, passphrase: &str) -> Result<Value, LockError> {
        let key = self.lock.begin_setup(passphrase)?;
        let dirs = self.wallet_dirs();
        for dir in &dirs {
            wallet_keys::stage_spend(dir, self.lock.vault(), &key)?;
        }
        let phrase = wallet_keys::read_phrase(&dirs[0], &key)?;
        let words = phrase.split_whitespace().count();
        let confirm = self.lock.finish_setup(&key, passphrase, words)?;
        self.retire_seeds(&dirs, &key);
        self.refresh_after_unlock();
        Ok(json!({
            "phrase": phrase.as_str(),
            "confirmWords": confirm,
            "lock": self.lock.status_value(),
        }))
    }

    /// `POST /unlock`, then move any seed made while locked into the spend tier.
    pub(crate) fn unlock_wallet(&self, passphrase: Option<&str>) -> Result<(), LockError> {
        let key = match passphrase {
            Some(passphrase) => self.lock.unlock(passphrase)?,
            None => self.lock.unlock_remembered()?,
        };
        let dirs = self.wallet_dirs();
        for dir in &dirs {
            if let Err(err) = wallet_keys::stage_spend(dir, self.lock.vault(), &key) {
                eprintln!("ob-node: {}: {err}", dir.display());
            }
        }
        self.retire_seeds(&dirs, &key);
        self.refresh_after_unlock();
        Ok(())
    }

    /// Sign the own store again if its record fell behind while locked.
    fn refresh_after_unlock(&self) {
        if let Err(err) = self.refresh_own_store() {
            eprintln!("ob-node: own store: {err}");
        }
    }

    fn retire_seeds(&self, dirs: &[PathBuf], key: &Zeroizing<[u8; 32]>) {
        for dir in dirs {
            if let Err(err) = wallet_keys::retire_seed(dir, self.lock.vault(), key) {
                eprintln!("ob-node: {}: {err}", dir.display());
            }
        }
    }

    /// `POST /setup/confirm`: the words at the positions setup asked for.
    pub(crate) fn confirm_backup(&self, words: &[String]) -> Result<(), LockError> {
        let key = self.lock.spend_key()?;
        let positions = self.lock.confirm_positions();
        if positions.is_empty() || positions.len() != words.len() {
            return Err(LockError::Invalid(format!("Type the words at positions {positions:?}.")));
        }
        let phrase = wallet_keys::read_phrase(&self.buyer_wallet().dir, &key)?;
        let all: Vec<&str> = phrase.split_whitespace().collect();
        let matches = positions
            .iter()
            .zip(words)
            .all(|(position, word)| all.get(position - 1).is_some_and(|expected| *expected == word.trim().to_lowercase()));
        if !matches {
            return Err(LockError::Invalid("Those words do not match the backup phrase.".into()));
        }
        self.lock.touch();
        self.lock.mark_backup_confirmed()
    }

    /// `POST /wallet/phrase`: the node wallet's phrase, after the passphrase again.
    pub(crate) fn reveal_phrase(&self, passphrase: &str) -> Result<Zeroizing<String>, LockError> {
        self.lock.verify(passphrase)?;
        let key = self.lock.spend_key()?;
        wallet_keys::read_phrase(&self.buyer_wallet().dir, &key).map_err(LockError::from)
    }

    /// Lock after the idle time or a sleep, every second until `stop` is set.
    pub(crate) fn run_auto_lock(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            if self.lock.tick() {
                eprintln!("ob-node: the wallet locked itself");
            }
            std::thread::sleep(AUTO_LOCK_POLL);
        }
    }
}
