//! The moderators this node knows, and the ones its stores accept.
//!
//! A moderator is its ruling key. A name is a label that anyone can copy, so
//! nothing here looks a moderator up by name. A profile enters the book only
//! after its signature and record verify ([`openbazaar_escrow::read_profile`]),
//! from the moderator's own address, from a store's node, or from this node's
//! own desk. The book keeps the newest profile per key.
//!
//! The accepted list is the owner's choice. It is signed into the own store's
//! record (`details.moderators`), and on testnet4 into the demo stores this
//! node serves, whose keys are public fixtures. A store with an empty list
//! does not offer moderated escrow. A new testnet4 node accepts the built-in
//! test moderator. Mainnet has no escrow: its catalog names no moderator, so
//! the list is empty there and cannot be changed.
//!
//! The track record is counted here, never claimed: the public rulings a
//! moderator serves are counted only when its ruling key signed them. The
//! built-in moderator's keys are public, so it gets no track record.
//!
//! `moderators.json` holds only public, signed data and the accepted ids.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use openbazaar_escrow::{read_profile, VerifiedProfile};
use openbazaar_protocol::{Catalog, ModeratorProfile, PublicRuling, Signed};
use serde_json::{json, Value};

use crate::escrow_fixture::{moderator_keys, moderator_mail_key, moderator_record};
use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::now_secs;
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::NetworkMode;
use crate::own_listings::StoreError;
use crate::peer_http::sale_base;
use crate::stores::unhex32;
use crate::{Error, Node};

const BOOK_FILE: &str = "moderators.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY: usize = 1024 * 1024;
/// The most public rulings one answer carries, and one fetch counts.
pub(crate) const MAX_RULINGS: usize = 1_000;
/// As many as a store record holds.
const MAX_ACCEPTED: usize = 16;
const BUILTIN_DESCRIPTION: &str = "The built-in test moderator. Its keys are public, so it is for testnet4 coins only.";

/// A verified profile, and what this node counted from the moderator's own
/// signed public rulings.
#[derive(Clone)]
pub(crate) struct Known {
    pub profile: VerifiedProfile,
    pub rulings: u32,
    pub stores: u32,
    /// Unix seconds of the last count from the moderator's address. 0: never.
    pub checked: u64,
}

pub(crate) struct Book {
    path: PathBuf,
    /// `None` until the owner changes the list: the network's default.
    accepted: Option<Vec<[u8; 32]>>,
    known: BTreeMap<[u8; 32], Known>,
}

impl Book {
    pub(crate) fn open(dir: &Path) -> Result<Self, Error> {
        let path = dir.join(BOOK_FILE);
        let value: Value = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| Error::Detail(format!("{BOOK_FILE}: {err}")))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(err) => return Err(Error::Detail(format!("{BOOK_FILE}: {err}"))),
        };
        let accepted = value["accepted"]
            .as_array()
            .map(|ids| ids.iter().filter_map(|id| unhex32(id.as_str()?)).collect());
        let mut known = BTreeMap::new();
        for entry in value["known"].as_array().into_iter().flatten() {
            let profile = entry["profile"].as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| read_profile(&bytes).ok());
            let Some(profile) = profile.filter(usable) else {
                eprintln!("ob-node: {BOOK_FILE}: a stored moderator profile no longer verifies and was dropped");
                continue;
            };
            let count = |key: &str| entry[key].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
            known.insert(
                profile.id(),
                Known { profile, rulings: count("rulings"), stores: count("stores"), checked: entry["checked"].as_u64().unwrap_or(0) },
            );
        }
        Ok(Self { path, accepted, known })
    }

    fn save(&self) -> Result<(), String> {
        let known: Vec<Value> = self
            .known
            .values()
            .map(|known| json!({ "profile": hex(&known.profile.bytes), "rulings": known.rulings, "stores": known.stores, "checked": known.checked }))
            .collect();
        let mut value = json!({ "known": known });
        if let Some(accepted) = &self.accepted {
            value["accepted"] = json!(accepted.iter().map(|id| hex(id)).collect::<Vec<_>>());
        }
        let tmp = self.path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(&value).expect("moderator json is finite");
        fs::write(&tmp, body).and_then(|_| fs::rename(&tmp, &self.path)).map_err(|err| err.to_string())
    }

    /// Keep `known` unless the book holds a newer profile for that key. A
    /// profile learned second hand (`checked` 0) keeps the counts on file.
    fn learn(&mut self, mut known: Known) -> bool {
        if let Some(old) = self.known.get(&known.profile.id()) {
            if old.profile.profile.updated > known.profile.profile.updated {
                return false;
            }
            if known.checked == 0 {
                (known.rulings, known.stores, known.checked) = (old.rulings, old.stores, old.checked);
            }
            if old.profile == known.profile && old.checked == known.checked {
                return false;
            }
        }
        self.known.insert(known.profile.id(), known);
        true
    }
}

/// A profile this node can use: its address is one a letter can be sent to.
fn usable(profile: &VerifiedProfile) -> bool {
    sale_base(&profile.profile.address).is_ok()
}

/// Count the public rulings in `list` that `moderator` signed: how many
/// distinct rulings, and how many distinct stores they name.
pub(crate) fn count_rulings(moderator: &[u8; 32], list: &[Value]) -> (u32, u32) {
    let mut rulings = BTreeSet::new();
    let mut stores = BTreeSet::new();
    for entry in list.iter().take(MAX_RULINGS) {
        let signed = entry.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| Signed::<PublicRuling>::decode(&bytes).ok());
        let Some(signed) = signed.filter(|signed| signed.terms.moderator_key == *moderator && signed.verify().is_ok()) else {
            continue;
        };
        rulings.insert(signed.terms.ruling_digest);
        stores.insert(signed.terms.store_key);
    }
    (rulings.len() as u32, stores.len() as u32)
}

/// Sign the demo stores in `catalog` again with `accepted` as their
/// moderators. Their keys are public fixtures, so this is testnet4 only.
fn sign_demo_stores(catalog: &mut Catalog, network: NetworkMode, accepted: &[[u8; 32]]) -> Result<(), Error> {
    if network != NetworkMode::Testnet4 {
        return Ok(());
    }
    for store in &mut catalog.stores {
        let Some(secret) = crate::demo_secret(store) else {
            continue;
        };
        let mut next = store.clone();
        let Some(details) = next.details.as_mut() else {
            continue;
        };
        if details.moderators == accepted {
            continue;
        }
        details.moderators = accepted.to_vec();
        *store = next.resign(&secret).map_err(Error::Protocol)?;
    }
    Ok(())
}

fn sorted(mut ids: Vec<[u8; 32]>) -> Vec<[u8; 32]> {
    ids.sort();
    ids.dedup();
    ids
}

/// The accepted moderators at boot, with the demo stores in `catalog` signed
/// for them. The default is the catalog's own moderator, if it has one.
pub(crate) fn accepted_at_boot(dir: &Path, catalog: &mut Catalog, network: NetworkMode) -> Result<Vec<[u8; 32]>, Error> {
    let builtin = (!catalog.moderator.name.is_empty()).then_some(catalog.moderator.id);
    let accepted = match (Book::open(dir)?.accepted, network) {
        (Some(accepted), NetworkMode::Testnet4) => sorted(accepted),
        _ => builtin.into_iter().collect(),
    };
    sign_demo_stores(catalog, network, &accepted)?;
    Ok(accepted)
}

impl Node {
    fn book(&self) -> std::sync::MutexGuard<'_, Book> {
        self.escrow.book.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// The built-in test moderator's id. `None` on mainnet.
    pub(crate) fn builtin_moderator(&self) -> Option<[u8; 32]> {
        moderator_record(self.network).ok().map(|record| record.id())
    }

    /// The built-in test moderator's profile, signed here with its public
    /// fixture key. Its address is the moderator this node was started with,
    /// or this node itself when it moderates.
    pub(crate) fn builtin_profile(&self) -> Option<VerifiedProfile> {
        let record = moderator_record(self.network).ok()?;
        let address = match &self.escrow.config.moderator_url {
            Some(url) => sale_base(url).ok()?,
            None if self.escrow.config.moderates => self.sale_reply_url()?,
            None => return None,
        };
        let name = self.catalog().moderator.name.clone();
        moderator_keys(self.network)
            .ok()?
            .sign_profile(ModeratorProfile {
                moderator_key: record.id(),
                updated: 0,
                name,
                description: BUILTIN_DESCRIPTION.into(),
                terms: String::new(),
                languages: vec!["en".into()],
                address,
                mailbox_key: *moderator_mail_key().as_bytes(),
                record: record.to_bytes(),
            })
            .ok()
    }

    /// What this node knows about moderator `id`: the built-in one, this
    /// node's own identity, or a profile in the book.
    pub(crate) fn known_moderator(&self, id: &[u8; 32]) -> Option<Known> {
        if self.builtin_moderator() == Some(*id) {
            return self.builtin_profile().map(|profile| Known { profile, rulings: 0, stores: 0, checked: 0 });
        }
        if let Some(profile) = self.own_moderator_profile().filter(|profile| profile.id() == *id) {
            let (rulings, stores) = count_rulings(id, &self.public_rulings_value());
            return Some(Known { profile, rulings, stores, checked: now_secs() });
        }
        self.book().known.get(id).cloned()
    }

    /// The moderators this node's stores accept, sorted: the owner's list on
    /// testnet4, else the catalog's own moderator, as [`accepted_at_boot`] has it.
    pub(crate) fn accepted_moderators(&self) -> Vec<[u8; 32]> {
        let builtin = {
            let catalog = self.catalog();
            (!catalog.moderator.name.is_empty()).then_some(catalog.moderator.id)
        };
        match (&self.book().accepted, self.network) {
            (Some(accepted), NetworkMode::Testnet4) => sorted(accepted.clone()),
            _ => builtin.into_iter().collect(),
        }
    }

    /// Fetch the profile a moderator publishes at `base`, and count the
    /// public rulings it serves.
    fn fetch_moderator(&self, base: &str) -> Result<Known, String> {
        let answer = crate::peer_http::call(&self.peers, "GET", &format!("{base}/public/moderator"), None, None, FETCH_TIMEOUT, MAX_BODY)?;
        if answer.status == 404 {
            return Err("That node does not publish a moderator profile. It may not moderate, or it runs an older build.".into());
        }
        if !(200..300).contains(&answer.status) {
            return Err(format!("{base}: {}", answer.status));
        }
        let value: Value = serde_json::from_slice(&answer.body).map_err(|_| "The moderator's answer could not be read.".to_string())?;
        let Some(text) = value["profile"].as_str() else {
            return Err("That node moderates, but its owner has not published a profile yet.".into());
        };
        let profile = unhex(text)
            .ok()
            .and_then(|bytes| read_profile(&bytes).ok())
            .ok_or("That node's moderator profile is not signed by the key it names.")?;
        if sale_base(&profile.profile.address).ok().as_deref() != Some(base) {
            return Err("That profile is published for another address. Use the address the moderator gives out.".into());
        }
        let (rulings, stores) = count_rulings(&profile.id(), value["rulings"].as_array().map_or(&[][..], Vec::as_slice));
        Ok(Known { profile, rulings, stores, checked: now_secs() })
    }

    /// Keep verified profiles a store's node relayed. Returns how many were new.
    pub(crate) fn learn_moderators(&self, list: &[Value]) -> usize {
        let own = self.own_moderator_id();
        let mut book = self.book();
        let mut learned = 0;
        for entry in list.iter().take(MAX_ACCEPTED) {
            let profile = entry.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| read_profile(&bytes).ok());
            let Some(profile) = profile.filter(usable) else {
                continue;
            };
            if Some(profile.id()) == own || Some(profile.id()) == self.builtin_moderator() {
                continue;
            }
            if book.learn(Known { profile, rulings: 0, stores: 0, checked: 0 }) {
                learned += 1;
            }
        }
        if learned > 0 {
            if let Err(err) = book.save() {
                eprintln!("ob-node: {BOOK_FILE}: {err}");
            }
        }
        learned
    }

    /// `POST /moderators`: accept a moderator for this node's stores, by the
    /// address it publishes its profile at (`{"address": …}`), or one this
    /// node already knows (`{"id": …}`). The list is signed into the store
    /// record, so the wallet must be unlocked.
    pub(crate) fn accept_moderator(&self, body: &Value) -> Result<Value, StoreError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(StoreError::invalid(ESCROW_MAINNET_REFUSAL));
        }
        self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let id = match (body["address"].as_str(), body["id"].as_str().and_then(unhex32)) {
            (Some(address), None) => {
                let base = sale_base(address).map_err(|_| StoreError::invalid("Paste the moderator's address: http:// and its .onion name."))?;
                if self.sale_reply_url().as_deref() == Some(base.as_str()) {
                    return Err(StoreError::invalid("That is this node. A store cannot use its own node as its moderator."));
                }
                let known = self.fetch_moderator(&base).map_err(StoreError::Invalid)?;
                let id = known.profile.id();
                if self.builtin_moderator() != Some(id) {
                    self.book().learn(known);
                }
                id
            }
            (None, Some(id)) => id,
            _ => return Err(StoreError::invalid("Send the moderator's address, or the id of one this node knows.")),
        };
        if Some(id) == self.own_moderator_id() {
            return Err(StoreError::invalid("That is this node's own moderator identity. A store cannot use its own node as its moderator."));
        }
        if self.known_moderator(&id).is_none() {
            return Err(StoreError::invalid("This node has no verified profile for that moderator. Add it by its address."));
        }
        let mut accepted = self.accepted_moderators();
        if !accepted.contains(&id) {
            if accepted.len() >= MAX_ACCEPTED {
                return Err(StoreError::invalid("A store accepts at most 16 moderators. Remove one first."));
            }
            accepted.push(id);
        }
        self.set_accepted(accepted)?;
        Ok(self.moderators_value())
    }

    /// `DELETE /moderators/{id}`: stop accepting a moderator for new orders.
    /// Escrows already opened with it keep it; its profile stays known.
    pub(crate) fn remove_moderator(&self, id: &str) -> Result<Value, StoreError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(StoreError::invalid(ESCROW_MAINNET_REFUSAL));
        }
        let id = unhex32(id).ok_or(StoreError::NotFound)?;
        self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let mut accepted = self.accepted_moderators();
        if !accepted.contains(&id) {
            return Err(StoreError::NotFound);
        }
        accepted.retain(|other| *other != id);
        self.set_accepted(accepted)?;
        Ok(self.moderators_value())
    }

    /// Store the accepted list, then sign it into the demo stores and the own store.
    fn set_accepted(&self, accepted: Vec<[u8; 32]>) -> Result<(), StoreError> {
        let accepted = sorted(accepted);
        {
            let mut book = self.book();
            book.accepted = Some(accepted.clone());
            book.save().map_err(StoreError::Failed)?;
        }
        sign_demo_stores(&mut self.catalog_mut(), self.network, &accepted).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.refresh_own_store().map_err(|err| StoreError::Failed(err.to_string()))
    }

    fn moderator_value(&self, known: &Known, accepted: bool) -> Value {
        let profile = &known.profile.profile;
        let builtin = self.builtin_moderator() == Some(known.profile.id());
        json!({
            "id": hex(&known.profile.id()),
            "name": profile.name,
            "description": profile.description,
            "terms": profile.terms,
            "languages": profile.languages,
            "feeSats": known.profile.record.fee.to_sat(),
            "address": profile.address,
            "builtIn": builtin,
            "accepted": accepted,
            // Counted from rulings the moderator's key signed. The built-in key is public, so nothing is counted for it.
            "trackRecord": (!builtin && known.checked > 0).then(|| json!({ "rulings": known.rulings, "stores": known.stores, "checkedAt": known.checked })),
        })
    }

    /// Moderator `id` as the shop lists it, or `None` when this node holds
    /// no profile for it. The built-in moderator is listed even when this
    /// node has no address for it: an escrow opens with it either way.
    fn listed_moderator(&self, id: &[u8; 32], accepted: bool) -> Option<Value> {
        if let Some(known) = self.known_moderator(id) {
            return Some(self.moderator_value(&known, accepted));
        }
        let record = moderator_record(self.network).ok().filter(|record| record.id() == *id)?;
        Some(json!({
            "id": hex(id),
            "name": self.catalog().moderator.name,
            "description": BUILTIN_DESCRIPTION,
            "terms": "",
            "languages": ["en"],
            "feeSats": record.fee.to_sat(),
            "address": null,
            "builtIn": true,
            "accepted": accepted,
            "trackRecord": null,
        }))
    }

    /// `GET /moderators`: the accepted moderators, then the others this node knows.
    pub(crate) fn moderators_value(&self) -> Value {
        let accepted = self.accepted_moderators();
        let own = self.own_moderator_id();
        let mut ids = accepted.clone();
        ids.extend(self.builtin_moderator());
        ids.extend(self.book().known.keys().copied());
        let mut seen = BTreeSet::new();
        let list: Vec<Value> = ids
            .into_iter()
            .filter(|id| seen.insert(*id) && Some(*id) != own)
            .filter_map(|id| self.listed_moderator(&id, accepted.contains(&id)))
            .collect();
        json!({ "moderators": list, "escrow": self.network == NetworkMode::Testnet4 })
    }

    /// `GET /stores/{key}/moderators`: the moderators a store's signed record
    /// accepts that this node holds a verified profile for. A buyer picks one.
    pub(crate) fn store_moderators_value(&self, key: &str) -> Option<Value> {
        let id = openbazaar_protocol::StoreId::from_bytes(unhex32(key)?);
        let accepted = self.catalog().store(&id)?.details.as_ref().map(|details| details.moderators.clone()).unwrap_or_default();
        let own = self.own_moderator_id();
        let list: Vec<Value> = accepted
            .iter()
            .filter(|id| Some(**id) != own)
            .filter_map(|id| self.listed_moderator(id, true))
            .collect();
        Some(json!({ "moderators": list, "unknown": accepted.len() - list.len() }))
    }

    /// The signed profiles of the moderators this node's stores accept, as
    /// `GET /public/moderators` and `GET /public/store` relay them.
    pub(crate) fn accepted_profiles(&self) -> Vec<String> {
        self.accepted_moderators()
            .iter()
            .filter_map(|id| self.known_moderator(id))
            .map(|known| hex(&known.profile.bytes))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbazaar_protocol::secret_from_label;

    fn ruling(label: &str, digest: u8, store: u8) -> Value {
        let secret = secret_from_label(label);
        let terms = PublicRuling {
            ruling_digest: [digest; 32],
            store_key: [store; 32],
            listing_id: [0xfe; 16],
            moderator_key: openbazaar_protocol::xonly(&secret),
        };
        json!(hex(&Signed::sign(terms, &secret).unwrap().encode().unwrap()))
    }

    #[test]
    fn only_rulings_the_moderator_signed_are_counted_once_each() {
        let moderator = openbazaar_protocol::xonly(&secret_from_label("track-record"));
        let mut forged: Vec<u8> = unhex(ruling("track-record", 0x91, 0xa1).as_str().unwrap()).unwrap();
        let last = forged.len() - 1;
        forged[last] ^= 1;
        let list = vec![
            ruling("track-record", 0x81, 0xa1),
            ruling("track-record", 0x81, 0xa1),
            ruling("track-record", 0x82, 0xa1),
            ruling("track-record", 0x83, 0xa2),
            ruling("someone-else", 0x84, 0xa3),
            json!(hex(&forged)),
            json!("not hex"),
            json!(7),
        ];
        assert_eq!(count_rulings(&moderator, &list), (3, 2));
        assert_eq!(count_rulings(&moderator, &[]), (0, 0));
    }
}
