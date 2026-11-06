//! Two key tiers for every node wallet.
//!
//! | tier    | file in `wallets/<name>/`        | sealed with            | holds |
//! |---------|----------------------------------|------------------------|-------|
//! | receive | `watch.obv`                      | vault key (keystore)   | public spend descriptors (BIP 86 xpubs), escrow mail key, silent payment code, store signing public key |
//! | spend   | `spend.obv`                      | spend key (`wallet.lock`, Argon2id) | the BIP 39 phrase: spend xprv, escrow keys, store signing, BIP 352 spend |
//!
//! The receive tier is open whenever the node runs, so a locked node still
//! watches the chain, hands out fresh addresses, reports balances, and opens
//! escrow mail. Nothing in it can sign a transaction or an escrow step.
//!
//! A data dir from before the lock keeps the phrase in `seed.obv` under the
//! vault key. That file stays until a passphrase is set; then the phrase is
//! copied into `spend.obv`, `wallet.lock` is written, and only after that is
//! `seed.obv` overwritten and deleted (see [`crate::wallet_lock`]).

use std::fs;
use std::path::Path;

use bdk_wallet::descriptor::IntoWalletDescriptor;
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use openbazaar_pay::SilentPaymentCode;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::network::NetworkMode;
use crate::seed::Seed;
use crate::vault::Vault;
use crate::vault_key::encode_hex;
use crate::Error;

pub(crate) const SEED_FILE: &str = "seed.obv";
pub(crate) const SPEND_FILE: &str = "spend.obv";
const WATCH_FILE: &str = "watch.obv";

/// The receive tier of one wallet.
pub(crate) struct WatchKeys {
    pub external: String,
    pub internal: String,
    /// Opens escrow mail sent to this wallet as buyer.
    pub mail_key: Zeroizing<[u8; 32]>,
    pub silent_payment_code: String,
    pub store_signing_pubkey: PublicKey,
}

impl WatchKeys {
    fn from_seed(seed: &Seed, network: NetworkMode) -> Result<Self, Error> {
        let secp = Secp256k1::new();
        let (external, internal) = seed.wallet_descriptors();
        let public = |descriptor: String| {
            descriptor
                .into_wallet_descriptor(&secp, network.bitcoin().into())
                .map(|(descriptor, _keys)| descriptor.to_string())
                .map_err(|err| Error::Detail(err.to_string()))
        };
        let code = SilentPaymentCode::from_keys(&seed.scan_key(), &seed.silent_spend_key(), network.bitcoin())
            .map_err(|err| Error::Detail(err.to_string()))?;
        Ok(Self {
            external: public(external)?,
            internal: public(internal)?,
            mail_key: Zeroizing::new(seed.mail_key().secret_bytes()),
            silent_payment_code: code.to_string(),
            store_signing_pubkey: PublicKey::from_secret_key(&secp, &seed.store_signing_key()),
        })
    }

    fn encode(&self) -> Vec<u8> {
        let value = json!({
            "version": 1,
            "external": self.external,
            "internal": self.internal,
            "mailKey": encode_hex(&self.mail_key[..]),
            "silentPaymentCode": self.silent_payment_code,
            "storeSigningPubkey": self.store_signing_pubkey.to_string(),
        });
        serde_json::to_vec(&value).expect("watch keys are plain json")
    }

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let bad = || Error::Detail("watch.obv could not be read".to_string());
        let value: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(bad);
        let mail = text("mailKey")?;
        let mut mail_key = Zeroizing::new([0u8; 32]);
        if mail.len() != 64 {
            return Err(bad());
        }
        for (index, byte) in mail_key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&mail[index * 2..index * 2 + 2], 16).map_err(|_| bad())?;
        }
        Ok(Self {
            external: text("external")?,
            internal: text("internal")?,
            mail_key,
            silent_payment_code: text("silentPaymentCode")?,
            store_signing_pubkey: text("storeSigningPubkey")?.parse().map_err(|_| bad())?,
        })
    }
}

/// The receive tier of the wallet in `dir`. A new wallet gets its phrase
/// here, in the vault tier (`seed.obv`), until the next passphrase setup or
/// unlock moves it into the spend tier.
pub(crate) fn open_watch(dir: &Path, vault: &Vault, network: NetworkMode) -> Result<WatchKeys, Error> {
    let path = dir.join(WATCH_FILE);
    if let Some(bytes) = vault.read_sealed(&path)? {
        return WatchKeys::decode(&bytes);
    }
    if dir.join(SPEND_FILE).exists() && !dir.join(SEED_FILE).exists() {
        return Err(Error::Detail(format!(
            "{} is missing and the wallet is locked. Restore it from a backup of the data dir.",
            path.display()
        )));
    }
    let seed = Seed::load_or_create(&dir.join(SEED_FILE), vault, network)?;
    let keys = WatchKeys::from_seed(&seed, network)?;
    vault.write_sealed(&path, &keys.encode())?;
    Ok(keys)
}

/// The phrase from the spend tier.
pub(crate) fn read_phrase(dir: &Path, spend_key: &[u8; 32]) -> Result<Zeroizing<String>, Error> {
    let bytes = Vault::from_key(spend_key)
        .read_sealed(&dir.join(SPEND_FILE))?
        .ok_or_else(|| Error::Detail("this wallet has no spend key file yet".to_string()))?;
    String::from_utf8(bytes)
        .map(Zeroizing::new)
        .map_err(|_| Error::Detail("the backup phrase could not be read".to_string()))
}

/// The seed from the spend tier, for one signing operation.
pub(crate) fn spend_seed(dir: &Path, spend_key: &[u8; 32], network: NetworkMode) -> Result<Seed, Error> {
    Seed::from_phrase(&read_phrase(dir, spend_key)?, network)
}

/// Copy a vault-tier `seed.obv` into `spend.obv` under `spend_key`. Leaves
/// `seed.obv` in place; [`retire_seed`] removes it once `wallet.lock` is
/// written. Returns `false` when there is nothing to move.
pub(crate) fn stage_spend(dir: &Path, vault: &Vault, spend_key: &[u8; 32]) -> Result<bool, Error> {
    let Some(phrase) = vault.read_sealed(&dir.join(SEED_FILE))?.map(Zeroizing::new) else {
        return Ok(false);
    };
    let spend = Vault::from_key(spend_key);
    let path = dir.join(SPEND_FILE);
    if let Ok(Some(existing)) = spend.read_sealed(&path) {
        if existing[..] == phrase[..] {
            return Ok(true);
        }
    }
    spend.write_sealed(&path, &phrase)?;
    if read_phrase(dir, spend_key)?.as_bytes() != &phrase[..] {
        return Err(Error::Detail("spend.obv did not read back".to_string()));
    }
    Ok(true)
}

/// Overwrite and delete `seed.obv` once `spend.obv` holds the same phrase.
pub(crate) fn retire_seed(dir: &Path, vault: &Vault, spend_key: &[u8; 32]) -> Result<(), Error> {
    let legacy = dir.join(SEED_FILE);
    let Some(phrase) = vault.read_sealed(&legacy)?.map(Zeroizing::new) else {
        return Ok(());
    };
    if read_phrase(dir, spend_key)?.as_bytes() != &phrase[..] {
        return Err(Error::Detail("spend.obv does not hold this wallet's phrase".to_string()));
    }
    let len = fs::metadata(&legacy).map(|meta| meta.len() as usize).unwrap_or(0);
    fs::write(&legacy, vec![0u8; len])
        .and_then(|()| fs::remove_file(&legacy))
        .map_err(|err| Error::Detail(format!("{} could not be removed: {err}", legacy.display())))
}

/// Remove a `spend.obv` left by a setup that never wrote `wallet.lock`. Its
/// key is gone, and `seed.obv` still holds the phrase.
pub(crate) fn drop_uncommitted(dir: &Path) -> Result<(), Error> {
    let spend = dir.join(SPEND_FILE);
    if spend.exists() && dir.join(SEED_FILE).exists() {
        fs::remove_file(&spend).map_err(|err| Error::Detail(err.to_string()))?;
    }
    Ok(())
}
