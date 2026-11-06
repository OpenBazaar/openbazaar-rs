//! The wallet lock: the spend key, unlock sessions, auto-lock, and backoff.
//!
//! The spend key is 256 random bits. It seals every wallet's `spend.obv`
//! ([`crate::wallet_keys`]) and is itself kept only in `wallet.lock`,
//! wrapped under Argon2id of the user's passphrase with
//! [`SPEND_COST`] (256 MiB, 3 passes, 1 lane; the costs sit in the
//! authenticated header). The node starts locked: the key is not in memory
//! until `POST /unlock`, and it is zeroized on lock.
//!
//! The node locks itself after [`State::Unlocked`] has seen no user action
//! for the auto-lock time (1 to 60 minutes, 15 by default). Idle time is
//! the larger of the monotonic and the wall clock, so time asleep counts,
//! and a wall clock that jumps more than [`SLEEP_GAP`] ahead of the
//! monotonic clock between two ticks (the machine slept) locks at once.
//!
//! Failed passphrases (unlock, step-up, reveal, settings) share one counter
//! sealed in `wallet-lock.obv`, so a restart does not reset it. After
//! [`FREE_FAILURES`] the next try waits 2, 4, 8, ... seconds, capped at an hour.
//!
//! A data dir with no `wallet.lock` is in [`State::Setup`]: every spend is
//! refused until a passphrase is set.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::spend_tickets::SpendTickets;
use crate::vault::Vault;
use crate::vault_key::{self, replace_file, unwrap_key, wrap_key, Cost, KeyStore, OsKeyStore, Unwrap, VaultMode};
use crate::Error;

pub(crate) const LOCK_FILE: &str = "wallet.lock";
const RECORD_FILE: &str = "wallet-lock.obv";
const REMEMBER_FILE: &str = "wallet.remember";
const MAGIC: &[u8; 4] = b"OBW1";
const KEY_CHECK_TAG: &[u8] = b"ob-node/wallet-key-check/v1";

/// Argon2id cost for `wallet.lock`: 256 MiB, 3 passes, 1 lane.
pub(crate) const SPEND_COST: Cost = Cost {
    memory_kib: 256 * 1024,
    passes: 3,
    lanes: 1,
};
#[cfg(test)]
const TEST_COST: Cost = Cost {
    memory_kib: 64,
    passes: 1,
    lanes: 1,
};

pub(crate) const DEFAULT_AUTO_LOCK_MINUTES: u32 = 15;
pub(crate) const MIN_AUTO_LOCK_MINUTES: u32 = 1;
pub(crate) const MAX_AUTO_LOCK_MINUTES: u32 = 60;
pub(crate) const MIN_PASSPHRASE_CHARS: usize = 8;
const MAX_PASSPHRASE_BYTES: usize = 1024;
pub(crate) const FREE_FAILURES: u32 = 2;
const MAX_DELAY_SECS: u64 = 3600;
pub(crate) const SLEEP_GAP: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// No passphrase yet (a new or pre-lock data dir).
    Setup,
    Locked,
    Unlocked,
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Setup => "setup",
            State::Locked => "locked",
            State::Unlocked => "unlocked",
        }
    }
}

#[derive(Debug)]
pub(crate) enum LockError {
    /// 423: the spend tier is closed.
    Locked(State),
    /// 401: the passphrase did not open `wallet.lock`.
    Wrong { retry_after: u64 },
    /// 429: too many wrong passphrases; try again later.
    Backoff { retry_after: u64 },
    Invalid(String),
    Failed(String),
}

impl From<Error> for LockError {
    fn from(err: Error) -> Self {
        LockError::Failed(err.to_string())
    }
}

/// Settings and the failure counter, sealed under the vault key.
#[derive(Default)]
struct Record {
    failures: u32,
    last_failure: u64,
    auto_lock_minutes: u32,
    /// The keystore account, or "file", holding a remembered spend key.
    remember: Option<String>,
    backup_confirmed: bool,
    /// 1-based word positions the user must type back after setup.
    confirm: Vec<usize>,
    key_check: Option<String>,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({
            "failures": self.failures,
            "lastFailure": self.last_failure,
            "autoLockMinutes": self.auto_lock_minutes,
            "remember": self.remember,
            "backupConfirmed": self.backup_confirmed,
            "confirm": self.confirm,
            "keyCheck": self.key_check,
        })
    }

    fn from_json(value: &Value) -> Self {
        let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        let minutes = u32::try_from(number("autoLockMinutes")).unwrap_or(0);
        Self {
            failures: u32::try_from(number("failures")).unwrap_or(u32::MAX),
            last_failure: number("lastFailure"),
            auto_lock_minutes: if (MIN_AUTO_LOCK_MINUTES..=MAX_AUTO_LOCK_MINUTES).contains(&minutes) {
                minutes
            } else {
                DEFAULT_AUTO_LOCK_MINUTES
            },
            remember: value.get("remember").and_then(Value::as_str).map(str::to_string),
            backup_confirmed: value.get("backupConfirmed").and_then(Value::as_bool).unwrap_or(false),
            confirm: value
                .get("confirm")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_u64).map(|n| n as usize).collect())
                .unwrap_or_default(),
            key_check: value.get("keyCheck").and_then(Value::as_str).map(str::to_string),
        }
    }

    fn retry_after(&self, now: u64) -> u64 {
        if self.failures <= FREE_FAILURES {
            return 0;
        }
        let delay = 1u64
            .checked_shl(self.failures - FREE_FAILURES)
            .unwrap_or(MAX_DELAY_SECS)
            .min(MAX_DELAY_SECS);
        (self.last_failure + delay).saturating_sub(now).min(delay)
    }
}

struct Session {
    key: Zeroizing<[u8; 32]>,
    last_mono: Instant,
    last_wall: Duration,
}

struct Inner {
    has_lock: bool,
    session: Option<Session>,
    record: Record,
    tick: Option<(Instant, Duration)>,
    skew_mono: Duration,
    skew_wall: Duration,
}

/// Where "remember on this device" keeps the spend key.
#[derive(Clone, Copy)]
enum Remember {
    Keystore,
    File,
    Unavailable,
}

pub(crate) struct WalletLock {
    dir: PathBuf,
    vault: Vault,
    remember: Remember,
    cost: Cost,
    inner: Mutex<Inner>,
    pub(crate) tickets: SpendTickets,
    /// Monero spend tickets ([`crate::xmr`]), dropped on lock with the rest.
    pub(crate) xmr_tickets: SpendTickets<crate::xmr::XmrTerms>,
}

impl WalletLock {
    pub(crate) fn open(dir: &Path, vault: Vault, mode: &VaultMode) -> Result<Self, Error> {
        let record = match vault.read_sealed(&dir.join(RECORD_FILE))? {
            Some(bytes) => Record::from_json(&serde_json::from_slice(&bytes).unwrap_or(Value::Null)),
            None => Record {
                auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
                ..Record::default()
            },
        };
        #[cfg(test)]
        let cost = TEST_COST;
        #[cfg(not(test))]
        let cost = SPEND_COST;
        Ok(Self {
            dir: dir.to_path_buf(),
            vault,
            remember: match mode {
                VaultMode::Keystore => Remember::Keystore,
                VaultMode::File => Remember::File,
                VaultMode::Passphrase(_) => Remember::Unavailable,
            },
            cost,
            inner: Mutex::new(Inner {
                has_lock: dir.join(LOCK_FILE).exists(),
                session: None,
                record,
                tick: None,
                skew_mono: Duration::ZERO,
                skew_wall: Duration::ZERO,
            }),
            tickets: SpendTickets::default(),
            xmr_tickets: SpendTickets::default(),
        })
    }

    pub(crate) fn vault(&self) -> &Vault {
        &self.vault
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// The monotonic clock the lock and the spend tickets run on.
    pub(crate) fn now(&self) -> Instant {
        Instant::now() + self.inner().skew_mono
    }

    fn clocks(inner: &Inner) -> (Instant, Duration) {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        (Instant::now() + inner.skew_mono, wall + inner.skew_wall)
    }

    /// Lock when the session has been idle for the auto-lock time.
    fn expire(&self, inner: &mut Inner) {
        let Some(session) = &inner.session else {
            return;
        };
        let (mono, wall) = Self::clocks(inner);
        let idle = mono
            .saturating_duration_since(session.last_mono)
            .max(wall.saturating_sub(session.last_wall));
        if idle >= Duration::from_secs(u64::from(inner.record.auto_lock_minutes) * 60) {
            self.close(inner);
        }
    }

    fn close(&self, inner: &mut Inner) {
        inner.session = None;
        self.tickets.clear();
        self.xmr_tickets.clear();
    }

    fn state_of(inner: &Inner) -> State {
        match (inner.has_lock, inner.session.is_some()) {
            (false, _) => State::Setup,
            (true, false) => State::Locked,
            (true, true) => State::Unlocked,
        }
    }

    pub(crate) fn state(&self) -> State {
        let mut inner = self.inner();
        self.expire(&mut inner);
        Self::state_of(&inner)
    }

    /// A copy of the spend key, or [`LockError::Locked`]. Does not count as
    /// user activity: background work after an unlock does not keep it open.
    pub(crate) fn spend_key(&self) -> Result<Zeroizing<[u8; 32]>, LockError> {
        let mut inner = self.inner();
        self.expire(&mut inner);
        match &inner.session {
            Some(session) => Ok(session.key.clone()),
            None => Err(LockError::Locked(Self::state_of(&inner))),
        }
    }

    /// The user did something. Restarts the auto-lock timer.
    pub(crate) fn touch(&self) {
        let mut inner = self.inner();
        self.expire(&mut inner);
        let (mono, wall) = Self::clocks(&inner);
        if let Some(session) = inner.session.as_mut() {
            session.last_mono = mono;
            session.last_wall = wall;
        }
    }

    /// One auto-lock round: idle timeout, and sleep detection. Returns `true`
    /// when this round locked the node.
    pub(crate) fn tick(&self) -> bool {
        let mut inner = self.inner();
        let was_open = inner.session.is_some();
        let (mono, wall) = Self::clocks(&inner);
        if let Some((last_mono, last_wall)) = inner.tick {
            let slept = wall.saturating_sub(last_wall).saturating_sub(mono.saturating_duration_since(last_mono));
            if slept > SLEEP_GAP {
                self.close(&mut inner);
            }
        }
        inner.tick = Some((mono, wall));
        self.expire(&mut inner);
        was_open && inner.session.is_none()
    }

    pub(crate) fn lock(&self) {
        let mut inner = self.inner();
        self.close(&mut inner);
    }

    fn read_lock(&self) -> Result<Vec<u8>, LockError> {
        std::fs::read(self.dir.join(LOCK_FILE)).map_err(|err| LockError::Failed(format!("wallet.lock: {err}")))
    }

    /// Unwrap `wallet.lock` with `passphrase`, counting a failure.
    fn check_passphrase(&self, passphrase: &str) -> Result<Zeroizing<[u8; 32]>, LockError> {
        {
            let inner = self.inner();
            if !inner.has_lock {
                return Err(LockError::Locked(State::Setup));
            }
            let retry_after = inner.record.retry_after(Self::clocks(&inner).1.as_secs());
            if retry_after > 0 {
                return Err(LockError::Backoff { retry_after });
            }
        }
        let blob = self.read_lock()?;
        let result = unwrap_key(MAGIC, &blob, passphrase).map(Zeroizing::new);
        let mut inner = self.inner();
        match result {
            Ok(key) => {
                if inner.record.failures != 0 {
                    inner.record.failures = 0;
                    self.save(&inner)?;
                }
                Ok(key)
            }
            Err(Unwrap::Wrong) => {
                inner.record.failures = inner.record.failures.saturating_add(1);
                inner.record.last_failure = Self::clocks(&inner).1.as_secs();
                self.save(&inner)?;
                Err(LockError::Wrong {
                    retry_after: inner.record.retry_after(inner.record.last_failure),
                })
            }
            Err(Unwrap::Unreadable) => Err(LockError::Failed("wallet.lock could not be read".into())),
        }
    }

    fn open_session(&self, inner: &mut Inner, key: &Zeroizing<[u8; 32]>) {
        let (mono, wall) = Self::clocks(inner);
        inner.session = Some(Session {
            key: key.clone(),
            last_mono: mono,
            last_wall: wall,
        });
    }

    /// `POST /unlock`. Returns the key so the node can move new seeds into the spend tier.
    pub(crate) fn unlock(&self, passphrase: &str) -> Result<Zeroizing<[u8; 32]>, LockError> {
        let key = self.check_passphrase(passphrase)?;
        let mut inner = self.inner();
        self.open_session(&mut inner, &key);
        Ok(key)
    }

    /// Step-up: the passphrase again, while unlocked.
    pub(crate) fn verify(&self, passphrase: &str) -> Result<(), LockError> {
        let open = self.spend_key()?;
        let key = self.check_passphrase(passphrase)?;
        if key[..] != open[..] {
            return Err(LockError::Failed("wallet.lock does not hold the open spend key".into()));
        }
        self.touch();
        Ok(())
    }

    /// Start a first passphrase: a fresh spend key, not yet written anywhere.
    pub(crate) fn begin_setup(&self, passphrase: &str) -> Result<Zeroizing<[u8; 32]>, LockError> {
        check_new_passphrase(passphrase)?;
        if self.inner().has_lock {
            return Err(LockError::Invalid("This wallet already has a passphrase.".into()));
        }
        Ok(Zeroizing::new(vault_key::random_bytes::<32>()))
    }

    /// Write `wallet.lock` for `key` (the commit point of setup) and open a session.
    /// Returns the word positions the user is asked to type back.
    pub(crate) fn finish_setup(&self, key: &Zeroizing<[u8; 32]>, passphrase: &str, words: usize) -> Result<Vec<usize>, LockError> {
        let blob = wrap_key(MAGIC, key, passphrase, self.cost)?;
        if unwrap_key(MAGIC, &blob, passphrase).ok().as_ref() != Some(&**key) {
            return Err(LockError::Failed("wallet.lock did not open with its own passphrase".into()));
        }
        let confirm = confirm_positions(words);
        let mut inner = self.inner();
        inner.record.key_check = Some(key_check(key));
        inner.record.confirm = confirm.clone();
        inner.record.backup_confirmed = false;
        inner.record.failures = 0;
        self.save(&inner)?;
        replace_file(&self.dir.join(LOCK_FILE), &blob)?;
        inner.has_lock = true;
        self.open_session(&mut inner, key);
        Ok(confirm)
    }

    pub(crate) fn confirm_positions(&self) -> Vec<usize> {
        self.inner().record.confirm.clone()
    }

    pub(crate) fn mark_backup_confirmed(&self) -> Result<(), LockError> {
        let mut inner = self.inner();
        inner.record.backup_confirmed = true;
        inner.record.confirm.clear();
        self.save(&inner).map_err(LockError::from)
    }

    /// Rewrap the spend key under `new`. The seeds are untouched: only
    /// `wallet.lock` changes, by an atomic rename.
    pub(crate) fn change_passphrase(&self, current: &str, new: &str) -> Result<(), LockError> {
        check_new_passphrase(new)?;
        self.verify(current)?;
        let key = self.spend_key()?;
        let blob = wrap_key(MAGIC, &key, new, self.cost)?;
        if unwrap_key(MAGIC, &blob, new).ok().as_ref() != Some(&*key) {
            return Err(LockError::Failed("the new wallet.lock did not open".into()));
        }
        replace_file(&self.dir.join(LOCK_FILE), &blob).map_err(LockError::from)
    }

    pub(crate) fn set_auto_lock(&self, minutes: u32) -> Result<(), LockError> {
        if !(MIN_AUTO_LOCK_MINUTES..=MAX_AUTO_LOCK_MINUTES).contains(&minutes) {
            return Err(LockError::Invalid(format!(
                "Auto-lock must be {MIN_AUTO_LOCK_MINUTES} to {MAX_AUTO_LOCK_MINUTES} minutes."
            )));
        }
        let mut inner = self.inner();
        inner.record.auto_lock_minutes = minutes;
        self.save(&inner).map_err(LockError::from)
    }

    /// Keep (or forget) the spend key on this device, so `POST /unlock`
    /// can open without the passphrase. Spends still need a step-up.
    pub(crate) fn set_remember(&self, on: bool) -> Result<(), LockError> {
        let key = self.spend_key()?;
        let mut inner = self.inner();
        let old = inner.record.remember.take();
        if let Some(account) = &old {
            self.forget(account)?;
        }
        if on {
            let account = match self.remember {
                Remember::Keystore => {
                    let account = format!("wallet-{}", vault_key::encode_hex(&vault_key::random_bytes::<16>()));
                    OsKeyStore.set(&account, &key).map_err(LockError::Failed)?;
                    account
                }
                Remember::File => {
                    replace_file(&self.dir.join(REMEMBER_FILE), &key[..])?;
                    "file".to_string()
                }
                Remember::Unavailable => {
                    self.save(&inner)?;
                    return Err(LockError::Invalid(
                        "This node keeps its vault behind a passphrase, so there is no OS keystore to remember the wallet in.".into(),
                    ));
                }
            };
            inner.record.remember = Some(account);
        }
        self.save(&inner).map_err(LockError::from)
    }

    fn forget(&self, account: &str) -> Result<(), LockError> {
        if account == "file" {
            let path = self.dir.join(REMEMBER_FILE);
            if path.exists() {
                std::fs::write(&path, [0u8; 32])
                    .and_then(|()| std::fs::remove_file(&path))
                    .map_err(|err| LockError::Failed(err.to_string()))?;
            }
            Ok(())
        } else {
            OsKeyStore.delete(account).map_err(LockError::Failed)
        }
    }

    /// `POST /unlock {"remembered": true}`.
    pub(crate) fn unlock_remembered(&self) -> Result<Zeroizing<[u8; 32]>, LockError> {
        let mut inner = self.inner();
        if !inner.has_lock {
            return Err(LockError::Locked(State::Setup));
        }
        let account = inner
            .record
            .remember
            .clone()
            .ok_or_else(|| LockError::Invalid("This device does not remember the wallet.".into()))?;
        let key = if account == "file" {
            let bytes = std::fs::read(self.dir.join(REMEMBER_FILE)).map_err(|err| LockError::Failed(err.to_string()))?;
            <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| LockError::Failed("wallet.remember is damaged".into()))?
        } else {
            OsKeyStore
                .get(&account)
                .map_err(LockError::Failed)?
                .ok_or_else(|| LockError::Failed("the OS keystore no longer has the remembered wallet".into()))?
        };
        let key = Zeroizing::new(key);
        if inner.record.key_check.as_deref() != Some(key_check(&key).as_str()) {
            return Err(LockError::Failed("the remembered key does not open this wallet".into()));
        }
        self.open_session(&mut inner, &key);
        Ok(key)
    }

    fn save(&self, inner: &Inner) -> Result<(), Error> {
        let bytes = serde_json::to_vec(&inner.record.to_json()).map_err(|err| Error::Detail(err.to_string()))?;
        self.vault.write_sealed(&self.dir.join(RECORD_FILE), &bytes)
    }

    /// `GET /lock`.
    pub(crate) fn status_value(&self) -> Value {
        let mut inner = self.inner();
        self.expire(&mut inner);
        let (mono, wall) = Self::clocks(&inner);
        let limit = Duration::from_secs(u64::from(inner.record.auto_lock_minutes) * 60);
        let lock_in = inner.session.as_ref().map(|session| {
            let idle = mono
                .saturating_duration_since(session.last_mono)
                .max(wall.saturating_sub(session.last_wall));
            limit.saturating_sub(idle).as_secs()
        });
        json!({
            "state": Self::state_of(&inner).as_str(),
            "autoLockMinutes": inner.record.auto_lock_minutes,
            "autoLockRange": [MIN_AUTO_LOCK_MINUTES, MAX_AUTO_LOCK_MINUTES],
            "lockInSecs": lock_in,
            "failedAttempts": inner.record.failures,
            "retryAfterSecs": inner.record.retry_after(wall.as_secs()),
            "rememberDevice": inner.record.remember.is_some(),
            "rememberAvailable": !matches!(self.remember, Remember::Unavailable),
            "backupConfirmed": inner.record.backup_confirmed,
            "minPassphraseChars": MIN_PASSPHRASE_CHARS,
        })
    }

    /// Move both clocks forward (auto-lock and ticket tests).
    #[cfg(test)]
    pub(crate) fn advance(&self, by: Duration) {
        let mut inner = self.inner();
        inner.skew_mono += by;
        inner.skew_wall += by;
    }

    /// Move only the wall clock (the machine slept, or a restart later).
    #[cfg(test)]
    pub(crate) fn advance_wall(&self, by: Duration) {
        self.inner().skew_wall += by;
    }
}

fn check_new_passphrase(passphrase: &str) -> Result<(), LockError> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(LockError::Invalid(format!(
            "Use a passphrase of at least {MIN_PASSPHRASE_CHARS} characters."
        )));
    }
    if passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(LockError::Invalid("That passphrase is too long.".into()));
    }
    Ok(())
}

fn key_check(key: &[u8; 32]) -> String {
    let mut engine = sha256::Hash::engine();
    engine.input(KEY_CHECK_TAG);
    engine.input(key);
    sha256::Hash::from_engine(engine).to_string()
}

/// Three distinct random word positions (1-based), in order.
fn confirm_positions(words: usize) -> Vec<usize> {
    let mut picked: Vec<usize> = Vec::new();
    while picked.len() < 3.min(words) {
        let [byte] = vault_key::random_bytes::<1>();
        let position = usize::from(byte) % words + 1;
        if !picked.contains(&position) {
            picked.push(position);
        }
    }
    picked.sort_unstable();
    picked
}
