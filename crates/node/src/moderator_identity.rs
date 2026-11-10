//! A moderating node's own identity (testnet4 only, `EscrowConfig::moderates`).
//!
//! The ruling key and the share key come from this node's seed
//! (`m/1984'/coin'/7'/0'`), so two nodes never share them and they survive a
//! restart with the backup phrase. They stay in the spend tier: nothing can
//! sign a ruling or a profile while the wallet is locked.
//!
//! What a locked node still needs is public or receive-tier, and is kept in
//! `moderator.obv` under the vault key: the mailbox secret that opens dispute
//! letters, the payout script, the profile text, the last signed profile,
//! and every record this identity has published. So a locked node serves its
//! profile, takes letters, and checks them against the record the parties
//! signed, including one from before the fee was changed. Changing the
//! profile and ruling need the wallet unlocked.
//!
//! The identity is created at the first unlock of a moderating node. Its
//! profile is first signed when the owner saves a name, and again whenever
//! the node's address changes while the wallet is unlocked.

use std::path::{Path, PathBuf};

use bitcoin::{Amount, ScriptBuf};
use openbazaar_escrow::{payout_dust_limit, read_profile, ModeratorKeys, ModeratorRecord, Policy, VerifiedProfile};
use openbazaar_protocol::{ModeratorProfile, MAX_MODERATOR_DESCRIPTION, MAX_MODERATOR_LANGUAGES, MAX_MODERATOR_TERMS, MAX_NAME};
use serde_json::{json, Value};
use tormail::StoreSecret;

use crate::escrow_fixture::MODERATOR_FEE_SATS;
use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::now_secs;
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::moderators::count_rulings;
use crate::network::NetworkMode;
use crate::own_listings::StoreError;
use crate::vault::Vault;
use crate::wallet_lock::State;
use crate::{Error, Node};

const IDENTITY_FILE: &str = "moderator.obv";
const PROFILE_FIELDS: &[&str] = &["name", "description", "terms", "languages", "feeSats"];

pub(crate) struct Identity {
    path: PathBuf,
    vault: Vault,
    mail_secret: [u8; 32],
    /// Where the fee is paid: one address of this node's wallet, taken once.
    payout: ScriptBuf,
    name: String,
    description: String,
    terms: String,
    languages: Vec<String>,
    fee_sats: u64,
    profile: Option<VerifiedProfile>,
    /// Every record this identity has published, oldest first.
    records: Vec<ModeratorRecord>,
    signing_key: [u8; 33],
}

impl Identity {
    /// The identity in `dir`, if this node has made one.
    pub(crate) fn open(dir: &Path, vault: &Vault) -> Result<Option<Self>, Error> {
        let path = dir.join(IDENTITY_FILE);
        let Some(bytes) = vault.read_sealed(&path)? else {
            return Ok(None);
        };
        let bad = || Error::Detail(format!("{IDENTITY_FILE} could not be read"));
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        let bytes_of = |key: &str| value[key].as_str().and_then(|text| unhex(text).ok());
        let text = |key: &str| value[key].as_str().unwrap_or("").to_string();
        let profile = match bytes_of("profile") {
            Some(bytes) => Some(read_profile(&bytes).map_err(|_| bad())?),
            None => None,
        };
        let records = value["records"]
            .as_array()
            .ok_or_else(bad)?
            .iter()
            .map(|record| record.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| ModeratorRecord::from_bytes(&bytes).ok()))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)?;
        Ok(Some(Self {
            path,
            vault: vault.clone(),
            mail_secret: bytes_of("mailSecret").and_then(|bytes| bytes.try_into().ok()).ok_or_else(bad)?,
            payout: ScriptBuf::from_bytes(bytes_of("payout").ok_or_else(bad)?),
            name: text("name"),
            description: text("description"),
            terms: text("terms"),
            languages: value["languages"].as_array().into_iter().flatten().filter_map(|tag| tag.as_str().map(str::to_string)).collect(),
            fee_sats: value["feeSats"].as_u64().ok_or_else(bad)?,
            profile,
            records,
            signing_key: bytes_of("signingKey").and_then(|bytes| bytes.try_into().ok()).ok_or_else(bad)?,
        }))
    }

    fn save(&self) -> Result<(), String> {
        let value = json!({
            "signingKey": hex(&self.signing_key),
            "mailSecret": hex(&self.mail_secret),
            "payout": hex(self.payout.as_bytes()),
            "name": self.name,
            "description": self.description,
            "terms": self.terms,
            "languages": self.languages,
            "feeSats": self.fee_sats,
            "profile": self.profile.as_ref().map(|profile| hex(&profile.bytes)),
            "records": self.records.iter().map(|record| hex(&record.to_bytes())).collect::<Vec<_>>(),
        });
        let body = serde_json::to_vec(&value).expect("identity json is finite");
        self.vault.write_sealed(&self.path, &body).map_err(|err| err.to_string())
    }

    fn id(&self) -> [u8; 32] {
        let mut id = [0u8; 32];
        id.copy_from_slice(&self.signing_key[1..]);
        id
    }

    /// Sign the profile for `address` with the current text and fee.
    fn sign(&mut self, keys: &ModeratorKeys, address: String) -> Result<(), StoreError> {
        let record = keys.record(self.payout.clone(), Amount::from_sat(self.fee_sats), Policy::default());
        if record.validate().is_err() {
            let least = payout_dust_limit(&self.payout).map_or(0, Amount::to_sat);
            return Err(StoreError::invalid(format!("The fee must be at least {least} sats, or the payout could not be spent.")));
        }
        let profile = keys
            .sign_profile(ModeratorProfile {
                moderator_key: record.id(),
                updated: now_secs(),
                name: self.name.clone(),
                description: self.description.clone(),
                terms: self.terms.clone(),
                languages: self.languages.clone(),
                address,
                mailbox_key: *StoreSecret::from_bytes(self.mail_secret).map_err(|err| StoreError::Failed(err.to_string()))?.public_key().as_bytes(),
                record: record.to_bytes(),
            })
            .map_err(|err| StoreError::invalid(format!("The profile could not be signed: {err}")))?;
        if !self.records.contains(&record) {
            self.records.push(record);
        }
        self.profile = Some(profile);
        Ok(())
    }
}

fn text_field(body: &Value, key: &str, max: usize, current: &str) -> Result<String, StoreError> {
    match body.get(key) {
        None => Ok(current.to_string()),
        Some(Value::String(text)) if text.trim().len() <= max => Ok(text.trim().to_string()),
        Some(_) => Err(StoreError::invalid(format!("The {key} is text of at most {max} bytes."))),
    }
}

impl Node {
    fn identity(&self) -> std::sync::MutexGuard<'_, Option<Identity>> {
        self.escrow.identity.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn moderates(&self) -> Result<(), StoreError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(StoreError::invalid(ESCROW_MAINNET_REFUSAL));
        }
        if !self.escrow.config.moderates {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// This node's moderator keys, from the spend tier.
    pub(crate) fn own_moderator_keys(&self) -> Result<ModeratorKeys, String> {
        let wallet = self.buyer_wallet();
        let seed = self.spend_seed(&wallet).map_err(|_| "the wallet is locked".to_string())?;
        ModeratorKeys::from_seed(&seed.moderator_seed()).map_err(|err| err.to_string())
    }

    pub(crate) fn own_moderator_id(&self) -> Option<[u8; 32]> {
        self.identity().as_ref().map(Identity::id)
    }

    /// The profile this node last signed for itself, if it has published one.
    pub(crate) fn own_moderator_profile(&self) -> Option<VerifiedProfile> {
        self.identity().as_ref().and_then(|identity| identity.profile.clone())
    }

    /// Every record this node's identity has published. A letter is checked
    /// against the one in its terms.
    pub(crate) fn own_moderator_records(&self) -> Vec<ModeratorRecord> {
        self.identity().as_ref().map(|identity| identity.records.clone()).unwrap_or_default()
    }

    /// Opens dispute letters sealed to this node's moderator profile.
    pub(crate) fn own_moderator_mail_secret(&self) -> Option<StoreSecret> {
        let secret = self.identity().as_ref()?.mail_secret;
        StoreSecret::from_bytes(secret).ok()
    }

    /// After an unlock, and while unlocked: make the identity if this
    /// moderating node has none, and sign the profile again if the node's
    /// address is not the one in it.
    pub(crate) fn refresh_moderator_identity(&self) {
        if !self.escrow.config.moderates || self.network != NetworkMode::Testnet4 || self.lock.state() != State::Unlocked {
            return;
        }
        let address = self.sale_reply_url();
        let current = match self.identity().as_ref() {
            None => false,
            Some(identity) => {
                let published = identity.profile.as_ref().map(|profile| profile.profile.address.as_str());
                identity.name.is_empty() || address.is_none() || published == address.as_deref()
            }
        };
        if current {
            return;
        }
        if let Err(err) = self.sign_moderator_identity(address, None) {
            eprintln!("ob-node: moderator identity: {}", match err {
                StoreError::Invalid(message) | StoreError::Failed(message) => message,
                StoreError::Locked(_) | StoreError::NotFound => "the wallet is locked".into(),
            });
        }
    }

    /// Create the identity if there is none, apply `edit`, sign the profile
    /// when it has a name and an address, and store the result.
    fn sign_moderator_identity(&self, address: Option<String>, edit: Option<&Value>) -> Result<(), StoreError> {
        let mut wallet = self.buyer_wallet();
        let seed = self.spend_seed(&wallet).map_err(|_| StoreError::Locked(self.lock.state().as_str()))?;
        let keys = ModeratorKeys::from_seed(&seed.moderator_seed()).map_err(|err| StoreError::Failed(err.to_string()))?;
        let mut slot = self.identity();
        // An identity left by a wallet this node no longer holds cannot sign anything: start again.
        let mut identity = match slot.take().filter(|identity| identity.signing_key == keys.signing_public()) {
            Some(identity) => identity,
            None => Identity {
                path: self.data_dir.join(IDENTITY_FILE),
                vault: self.lock.vault().clone(),
                mail_secret: seed.moderator_mailbox_secret(),
                payout: wallet.fresh_address().map_err(|err| StoreError::Failed(err.to_string()))?.script_pubkey(),
                name: String::new(),
                description: String::new(),
                terms: String::new(),
                languages: Vec::new(),
                fee_sats: MODERATOR_FEE_SATS,
                profile: None,
                records: Vec::new(),
                signing_key: keys.signing_public(),
            },
        };
        drop(wallet);
        let result = (|| {
            if let Some(body) = edit {
                identity.name = text_field(body, "name", MAX_NAME, &identity.name)?;
                identity.description = text_field(body, "description", MAX_MODERATOR_DESCRIPTION, &identity.description)?;
                identity.terms = text_field(body, "terms", MAX_MODERATOR_TERMS, &identity.terms)?;
                if let Some(languages) = body.get("languages") {
                    identity.languages = languages_of(languages)?;
                }
                if let Some(fee) = body.get("feeSats") {
                    identity.fee_sats = fee.as_u64().ok_or_else(|| StoreError::invalid("The fee is a whole number of sats."))?;
                }
                if identity.name.is_empty() {
                    return Err(StoreError::invalid("Give the moderator a name. Buyers and sellers see it next to its key."));
                }
            }
            match address {
                Some(address) if !identity.name.is_empty() => identity.sign(&keys, address)?,
                None if edit.is_some() => {
                    return Err(StoreError::invalid(
                        "This node has no address other nodes can reach yet. Wait for its onion address, then save again.",
                    ))
                }
                _ => {}
            }
            identity.save().map_err(StoreError::Failed)
        })();
        match result {
            Ok(()) => {
                *slot = Some(identity);
                Ok(())
            }
            Err(err) => {
                // Nothing was stored: put back what is on disk.
                *slot = Identity::open(&self.data_dir, self.lock.vault()).ok().flatten();
                Err(err)
            }
        }
    }

    /// `PUT /moderator/profile`: change the name, description, terms,
    /// languages or fee, and sign the profile again. Needs the wallet unlocked.
    pub(crate) fn update_moderator_profile(&self, body: &Value) -> Result<Value, StoreError> {
        self.moderates()?;
        let known = body.as_object().is_some_and(|map| !map.is_empty() && map.keys().all(|key| PROFILE_FIELDS.contains(&key.as_str())));
        if !known {
            return Err(StoreError::invalid("Send the name, description, terms, languages or feeSats to change."));
        }
        self.lock.spend_key().map_err(|_| StoreError::Locked(self.lock.state().as_str()))?;
        self.sign_moderator_identity(self.sale_reply_url(), Some(body))?;
        self.moderator_profile_value()
    }

    /// `GET /moderator/profile`: this node's identity as its desk shows it.
    pub(crate) fn moderator_profile_value(&self) -> Result<Value, StoreError> {
        self.moderates()?;
        let builtin = self.builtin_moderator().map(|id| json!({ "id": hex(&id), "name": self.catalog().moderator.name }));
        let address = self.sale_reply_url();
        let Some(id) = self.own_moderator_id() else {
            return Ok(json!({ "id": null, "published": false, "address": address, "builtIn": builtin }));
        };
        let (rulings, stores) = count_rulings(&id, &self.public_rulings_value());
        let slot = self.identity();
        let identity = slot.as_ref().ok_or(StoreError::NotFound)?;
        Ok(json!({
            "id": hex(&id),
            "name": identity.name,
            "description": identity.description,
            "terms": identity.terms,
            "languages": identity.languages,
            "feeSats": identity.fee_sats,
            "minFeeSats": payout_dust_limit(&identity.payout).map_or(0, Amount::to_sat),
            "published": identity.profile.is_some(),
            // The address in the signed profile, and the one this node has now.
            "publishedAddress": identity.profile.as_ref().map(|profile| profile.profile.address.clone()),
            "address": address,
            "trackRecord": { "rulings": rulings, "stores": stores },
            "builtIn": builtin,
        }))
    }

    /// `GET /public/moderator`: the signed profile and the public rulings.
    pub(crate) fn public_moderator_value(&self) -> Option<Value> {
        if !self.escrow.config.moderates || self.network != NetworkMode::Testnet4 {
            return None;
        }
        Some(json!({
            "profile": self.own_moderator_profile().map(|profile| hex(&profile.bytes)),
            "rulings": self.public_rulings_value(),
        }))
    }
}

fn languages_of(value: &Value) -> Result<Vec<String>, StoreError> {
    let bad = || StoreError::invalid("Languages are up to 8 tags such as en or pt-br.");
    let list = value.as_array().ok_or_else(bad)?;
    let mut out = Vec::new();
    for tag in list {
        let tag = tag.as_str().ok_or_else(bad)?.trim().to_ascii_lowercase();
        if !(2..=12).contains(&tag.len()) || !tag.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'-') {
            return Err(bad());
        }
        if !out.contains(&tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_MODERATOR_LANGUAGES {
        return Err(bad());
    }
    Ok(out)
}
