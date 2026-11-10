//! The node's own store record, across the two key tiers.
//!
//! Everything in the record except its signature comes from the receive tier
//! and public files: the store key's public half, the silent payment code,
//! and the mailbox key from `watch.obv`, and the profile from
//! `store-profile.json`. Only the signature needs the store signing key,
//! which lives in the spend tier (`spend.obv`) with the rest of the phrase.
//!
//! The store key stays in the spend tier because nothing in the record has
//! to change while the wallet is locked: the keys are fixed by the seed, and
//! the profile, moderators, rating, and Monero choice change only through
//! routes that need an unlocked wallet (or by editing the profile file,
//! which takes effect at the next unlock). So the node keeps the last record
//! it signed in `store-record.obv` (receive tier) and serves that while
//! locked. When the inputs no longer match it, the old record stays up and
//! is signed again at the next unlock ([`Node::refresh_own_store`]).
//!
//! Before the first passphrase the phrase is still in the vault tier
//! (`seed.obv`), and the record is signed from that.

use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{ContentRating, CountryCode, Store, StoreDetails, StoreId, StoreProfile};
use serde_json::{json, Value};
use tormail::StoreSecret;

use crate::network::NetworkMode;
use crate::stores::direct_pay;
use crate::vault::Vault;
use crate::wallet::NodeWallet;
use crate::wallet_keys;
use crate::wallet_lock::State;
use crate::{Error, Node};

pub(crate) const PROFILE_FILE: &str = "store-profile.json";
const RECORD_FILE: &str = "store-record.obv";
const DEFAULT_NAME: &str = "My store";

/// What [`open`] found: the record to serve, and whether it is behind its inputs.
pub(crate) struct Opened {
    pub id: StoreId,
    pub record: Option<Store>,
    pub pending: bool,
}

/// The x-only store key of `wallet`, from the receive tier.
pub(crate) fn store_id(wallet: &NodeWallet) -> StoreId {
    let (xonly, _) = wallet.watch.store_signing_pubkey.x_only_public_key();
    StoreId::from_bytes(xonly.serialize())
}

/// The payment rails a store record opts in to besides on-chain bitcoin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rails {
    pub xmr: bool,
    pub lightning: bool,
}

impl Rails {
    /// What `store` signed, or neither.
    pub(crate) fn of(store: Option<&Store>) -> Self {
        let details = store.and_then(|store| store.details.as_ref());
        Self {
            xmr: details.is_some_and(|d| d.accepts_xmr),
            lightning: details.is_some_and(|d| d.accepts_lightning),
        }
    }
}

/// The record this node would sign now, without its signature. `None` while
/// `watch.obv` predates the store mailbox key.
pub(crate) fn unsigned(
    dir: &Path,
    wallet: &NodeWallet,
    network: NetworkMode,
    moderators: &[[u8; 32]],
    rating: ContentRating,
    rails: Rails,
) -> Result<Option<Store>, Error> {
    let Ok(mailbox) = wallet.watch.store_mailbox_key() else {
        return Ok(None);
    };
    let mailbox = StoreSecret::from_bytes(*mailbox).map_err(|err| Error::Detail(err.to_string()))?;
    let profile = read_profile(dir)?;
    let mut ships_to = Vec::new();
    for code in profile.get("shipsTo").and_then(Value::as_array).into_iter().flatten() {
        let code = code.as_str().unwrap_or("");
        ships_to.push(CountryCode::parse(code).map_err(|err| Error::Detail(format!("{PROFILE_FILE}: {err}")))?);
    }
    ships_to.sort();
    ships_to.dedup();
    let mut moderators = moderators.to_vec();
    moderators.sort();
    moderators.dedup();
    let created_on = profile
        .get("createdOn")
        .and_then(Value::as_u64)
        .and_then(|day| u32::try_from(day).ok())
        .unwrap_or_else(today);
    let mut name = text(&profile, "name");
    if name.is_empty() {
        name = DEFAULT_NAME.to_string();
    }
    let watch = &wallet.watch;
    let direct_pay = direct_pay(watch.store_signing_pubkey, watch.silent_payment_code.clone(), &name, network)?;
    Ok(Some(Store {
        name,
        region: text(&profile, "region"),
        direct_pay,
        id: store_id(wallet),
        rating: Some(rating),
        details: Some(StoreDetails {
            mailbox_key: *mailbox.public_key().as_bytes(),
            onion: text(&profile, "onion"),
            moderators,
            profile: StoreProfile {
                tagline: text(&profile, "tagline"),
                about: text(&profile, "about"),
                avatar: image(&profile, "avatar")?,
                banner: image(&profile, "banner")?,
                shipping_policy: text(&profile, "shippingPolicy"),
                returns_policy: text(&profile, "returnsPolicy"),
                processing_time: text(&profile, "processingTime"),
                ships_to,
                created_on,
            },
            accepts_xmr: rails.xmr,
            accepts_lightning: rails.lightning && network != NetworkMode::Mainnet,
        }),
        signature: [0u8; 64],
    }))
}

/// Sign `store` with `secret`; the free-text check runs here.
fn sign(store: Store, secret: &SecretKey) -> Result<Store, Error> {
    store.resign(secret).map_err(|err| Error::Detail(format!("{PROFILE_FILE}: {err}")))
}

/// `true` when `record` is `unsigned` with a signature.
fn current(record: &Store, unsigned: &Store) -> bool {
    *record == Store { signature: record.signature, ..unsigned.clone() }
}

fn read_cached(dir: &Path, vault: &Vault, id: &StoreId) -> Result<Option<Store>, Error> {
    let Some(bytes) = vault.read_sealed(&dir.join(RECORD_FILE))? else {
        return Ok(None);
    };
    let store = Store::decode(&bytes).map_err(Error::Protocol)?;
    store.verify().map_err(Error::Protocol)?;
    Ok((store.id == *id).then_some(store))
}

pub(crate) fn write_cached(dir: &Path, vault: &Vault, store: &Store) -> Result<(), Error> {
    vault.write_sealed(&dir.join(RECORD_FILE), &store.encode().map_err(Error::Protocol)?)
}

/// The own store at boot: the cached record if it is current, a fresh one if
/// the phrase is still in the vault tier, else the cached one marked pending.
pub(crate) fn open(
    dir: &Path,
    vault: &Vault,
    wallet: &NodeWallet,
    network: NetworkMode,
    moderators: &[[u8; 32]],
) -> Result<Opened, Error> {
    let id = store_id(wallet);
    let cached = read_cached(dir, vault, &id)?;
    let rating = cached.as_ref().and_then(|store| store.rating).unwrap_or(ContentRating::General);
    let want = unsigned(dir, wallet, network, moderators, rating, Rails::of(cached.as_ref()))?;
    let seed = wallet_keys::vault_tier_seed(&wallet.dir, vault, network)?;
    let (record, pending) = match (want, cached, seed) {
        (Some(want), Some(cached), _) if current(&cached, &want) => (Some(cached), false),
        (Some(want), _, Some(seed)) => {
            let store = sign(want, &seed.store_signing_key())?;
            write_cached(dir, vault, &store)?;
            (Some(store), false)
        }
        // `watch.obv` predates the mailbox key: nothing to compare against
        // until the unlock that upgrades it.
        (None, cached, _) => (cached, false),
        (Some(_), cached, _) => (cached, true),
    };
    Ok(Opened { id, record, pending })
}

impl Node {
    /// The own store's signing key: from the spend tier while unlocked, or
    /// from `seed.obv` before the first passphrase. `None` while locked.
    pub(crate) fn own_signing_key(&self) -> Option<SecretKey> {
        let wallet = self.buyer_wallet();
        if self.lock.state() == State::Unlocked {
            return self.spend_seed(&wallet).ok().map(|seed| seed.store_signing_key());
        }
        wallet_keys::vault_tier_seed(&wallet.dir, self.lock.vault(), self.network)
            .ok()
            .flatten()
            .map(|seed| seed.store_signing_key())
    }

    /// The own store's record as served now, if it was ever signed.
    pub(crate) fn own_record(&self) -> Option<Store> {
        self.catalog().store(&self.own_store).cloned()
    }

    /// Sign the own store again if its inputs changed, after an unlock. Also
    /// upgrades a `watch.obv` from before the store mailbox key.
    pub(crate) fn refresh_own_store(&self) -> Result<(), Error> {
        let Some(secret) = self.own_signing_key() else {
            return Ok(());
        };
        {
            let mut wallet = self.buyer_wallet();
            if wallet.watch.needs_upgrade() {
                let seed = self.spend_seed(&wallet).map_err(|err| Error::Detail(format!("{err:?}")))?;
                let dir = wallet.dir.clone();
                wallet_keys::upgrade_watch(&dir, self.lock.vault(), &mut wallet.watch, &seed)?;
            }
        }
        let old = self.own_record();
        let rating = old.as_ref().and_then(|store| store.rating).unwrap_or(ContentRating::General);
        self.publish_own(rating, Rails::of(old.as_ref()), &secret)
    }

    /// Sign the own store with `rating` and `rails` unless the served
    /// record already says that, then serve and cache it.
    pub(crate) fn publish_own(&self, rating: ContentRating, rails: Rails, secret: &SecretKey) -> Result<(), Error> {
        let moderators = self.own_moderators();
        let want = {
            let wallet = self.buyer_wallet();
            unsigned(&self.data_dir, &wallet, self.network, &moderators, rating, rails)?
        };
        let Some(want) = want else {
            return Ok(());
        };
        if self.own_record().is_some_and(|record| current(&record, &want)) {
            self.own_pending.store(false, Ordering::Relaxed);
            return Ok(());
        }
        let store = sign(want, secret)?;
        write_cached(&self.data_dir, self.lock.vault(), &store)?;
        self.serve_own(store);
        Ok(())
    }

    /// Put `store` in the catalog in place of the own store's older record.
    pub(crate) fn serve_own(&self, store: Store) {
        let mut catalog = self.catalog_mut();
        match catalog.stores.iter_mut().find(|old| old.id == store.id) {
            Some(old) => *old = store,
            None => catalog.stores.push(store),
        }
        self.own_pending.store(false, Ordering::Relaxed);
    }

    /// The moderators the own store accepts: the owner's list ([`crate::moderators`]).
    pub(crate) fn own_moderators(&self) -> Vec<[u8; 32]> {
        self.accepted_moderators()
    }
}

pub(crate) fn today_yyyymmdd() -> u32 {
    today()
}

/// Today as YYYYMMDD in UTC.
pub(crate) fn today() -> u32 {
    let days = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year * 10_000 + month * 100 + day) as u32
}

/// `store-profile.json`, created with a default name and today's date on first boot.
pub(crate) fn read_profile(dir: &Path) -> Result<Value, Error> {
    let path = dir.join(PROFILE_FILE);
    match fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str(&body).map_err(|err| Error::Detail(format!("{PROFILE_FILE}: {err}"))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let value = json!({ "name": DEFAULT_NAME, "createdOn": today() });
            let body = serde_json::to_vec_pretty(&value).expect("profile json is finite");
            fs::write(&path, body).map_err(|err| Error::Detail(err.to_string()))?;
            Ok(value)
        }
        Err(err) => Err(Error::Detail(err.to_string())),
    }
}

/// Replace `store-profile.json` with `profile`.
pub(crate) fn write_profile(dir: &Path, profile: &Value) -> Result<(), Error> {
    let path = dir.join(PROFILE_FILE);
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(profile).expect("profile json is finite");
    fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, &path)).map_err(|err| Error::Detail(err.to_string()))
}

fn text(profile: &Value, key: &str) -> String {
    profile.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn image(profile: &Value, key: &str) -> Result<Option<[u8; 32]>, Error> {
    let bad = || Error::Detail(format!("{PROFILE_FILE}: {key} is not a SHA-256 hex hash"));
    match profile.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(hash)) => crate::stores::unhex32(hash).map(Some).ok_or_else(bad),
        Some(_) => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_is_a_calendar_date() {
        let day = today();
        assert!(day >= 20260101 && (1..=12).contains(&(day / 100 % 100)) && (1..=31).contains(&(day % 100)));
    }
}
