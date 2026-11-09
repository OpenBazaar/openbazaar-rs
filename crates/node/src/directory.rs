//! Follow-only replication.
//!
//! A peer asks each introduction for the seed document and keeps the highest
//! sequence that verifies under the shipped key. It then stores the signed
//! store and listing records for the document's allowlist and for store keys
//! a person added. A source that is down is skipped. Records already on disk
//! stay, so a publisher can exit and a later open still serves the listing.
//!
//! Nothing here accepts a client-supplied onion. Add-store takes a store key.
//! The record is kept only when its signature matches that key.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use openbazaar_protocol::{Catalog, Listing, SeedDocument, Store, StoreId};
use serde_json::{json, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::orders::OrderError;
use crate::seeds::DOCUMENT_KEY;
use crate::{listing_value, Error, Node};

/// One compiled introduction. A checksum that Arti refuses is "down": the
/// pull skips it and keeps what is already stored.
struct Introduction {
    onion: String,
    state_dir: PathBuf,
}

impl Source for Introduction {
    fn seed_bytes(&self) -> Result<Vec<u8>, String> {
        ask(&self.state_dir, &self.onion, tormail::QUERY_SEED, [0u8; 32])?
            .ok_or_else(|| "introduction has no document".into())
    }

    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, tormail::QUERY_STORE, *id)
    }

    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        match ask(&self.state_dir, &self.onion, tormail::QUERY_LISTINGS, *id)? {
            Some(blob) => split_listings(&blob),
            None => Ok(Vec::new()),
        }
    }
}

fn ask(state_dir: &Path, onion: &str, kind: u8, id: [u8; 32]) -> Result<Option<Vec<u8>>, String> {
    let onion = tormail::OnionAddress::parse(onion).map_err(|err| err.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(async {
            let client = tormail::open_client(state_dir)?;
            tokio::time::timeout(Duration::from_secs(30), tormail::ask_directory(&client, &onion, kind, id))
                .await
                .map_err(|_| tormail::Error::Transport("introduction timed out"))?
        })
        .map_err(|err| err.to_string())
}

fn split_listings(blob: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < blob.len() {
        if blob.len() - at < 4 {
            return Err("listing list was cut off".into());
        }
        let len = u32::from_le_bytes(blob[at..at + 4].try_into().expect("4 bytes")) as usize;
        at += 4;
        if len > 1_048_576 || blob.len() - at < len {
            return Err("listing list was cut off".into());
        }
        out.push(blob[at..at + len].to_vec());
        at += len;
    }
    Ok(out)
}

/// One introduction, or any other place a signed document and records are read from.
pub(crate) trait Source {
    fn seed_bytes(&self) -> Result<Vec<u8>, String>;
    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String>;
    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String>;
}

pub(crate) struct Replica {
    dir: PathBuf,
    document: Option<SeedDocument>,
    followed: BTreeSet<[u8; 32]>,
    stores: BTreeMap<[u8; 32], Store>,
    listings: BTreeMap<[u8; 32], Listing>,
}

impl Replica {
    pub(crate) fn open(dir: &Path) -> Result<Self, Error> {
        let dir = dir.join("directory");
        fs::create_dir_all(dir.join("stores")).map_err(|err| Error::Detail(err.to_string()))?;
        fs::create_dir_all(dir.join("listings")).map_err(|err| Error::Detail(err.to_string()))?;
        let mut replica = Self {
            dir,
            document: None,
            followed: BTreeSet::new(),
            stores: BTreeMap::new(),
            listings: BTreeMap::new(),
        };
        replica.load().map_err(|err| Error::Detail(err))?;
        Ok(replica)
    }

    /// Ask `sources` for a newer document and for the records it names.
    /// A source that returns an error is skipped. A bad signature is skipped.
    /// An older sequence does not replace the document already held.
    pub(crate) fn pull(&mut self, sources: &[&dyn Source], key: &[u8; 32]) -> Result<(), String> {
        let mut best = self.document.clone();
        for source in sources {
            let bytes = match source.seed_bytes() {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let document = match SeedDocument::decode(&bytes) {
                Ok(document) => document,
                Err(_) => continue,
            };
            if document.verify_signer(key).is_err() {
                continue;
            }
            let newer = best.as_ref().map(|held| document.sequence > held.sequence).unwrap_or(true);
            if newer {
                best = Some(document);
            }
        }
        if let Some(document) = best {
            if self.document.as_ref().map(|held| held.sequence) != Some(document.sequence) {
                self.document = Some(document);
                self.save_document()?;
                self.prune()?;
            }
        }
        let mut keys = self.document.as_ref().map(|document| document.stores.clone()).unwrap_or_default();
        for followed in &self.followed {
            if !keys.contains(followed) {
                keys.push(*followed);
            }
        }
        if keys.is_empty() {
            return Ok(());
        }
        for id in keys {
            for source in sources {
                if let Ok(Some(bytes)) = source.store_bytes(&id) {
                    if let Ok(store) = Store::decode(&bytes) {
                        if store.verify().is_ok() && store.id.as_bytes() == &id {
                            self.save_store(&store)?;
                        }
                    }
                }
                if let Ok(lists) = source.listing_bytes(&id) {
                    for bytes in lists {
                        if let Ok(listing) = Listing::decode(&bytes) {
                            if listing.verify().is_ok() && listing.store_id.as_bytes() == &id {
                                if let Ok(hash) = listing.content_hash() {
                                    self.save_listing(*hash.as_bytes(), &listing)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn answer(&self, kind: u8, id: &[u8; 32]) -> Option<Vec<u8>> {
        match kind {
            tormail::QUERY_SEED => self.document.as_ref()?.encode().ok(),
            tormail::QUERY_STORE => self.stores.get(id)?.encode().ok(),
            tormail::QUERY_LISTINGS => {
                let mut out = Vec::new();
                for listing in self.listings.values() {
                    if listing.store_id.as_bytes() != id {
                        continue;
                    }
                    let bytes = listing.encode().ok()?;
                    let len = u32::try_from(bytes.len()).ok()?;
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(&bytes);
                }
                Some(out)
            }
            _ => None,
        }
    }

    pub(crate) fn follow(&mut self, id: [u8; 32]) -> Result<(), String> {
        if self.followed.insert(id) {
            self.save_followed()?;
        }
        Ok(())
    }

    /// Put replicated records into `catalog`. The node's own store record
    /// is left as this process signed it.
    pub(crate) fn apply(&self, catalog: &mut Catalog, own: StoreId) {
        for store in self.stores.values() {
            if store.id == own {
                continue;
            }
            match catalog.stores.iter_mut().find(|have| have.id == store.id) {
                Some(have) => *have = store.clone(),
                None => catalog.stores.push(store.clone()),
            }
        }
        for listing in self.listings.values() {
            let hash = listing.content_hash().ok();
            let present = catalog.listings.iter().any(|have| have.content_hash().ok() == hash);
            if !present {
                catalog.listings.push(listing.clone());
            }
        }
    }

    fn wanted(&self, id: &[u8; 32]) -> bool {
        self.followed.contains(id) || self.document.as_ref().is_some_and(|document| document.allows(id))
    }

    fn prune(&mut self) -> Result<(), String> {
        let drop_stores: Vec<[u8; 32]> = self.stores.keys().copied().filter(|id| !self.wanted(id)).collect();
        for id in drop_stores {
            self.stores.remove(&id);
            let _ = fs::remove_file(self.store_path(&id));
        }
        let drop_listings: Vec<[u8; 32]> = self
            .listings
            .iter()
            .filter(|(_, listing)| !self.wanted(listing.store_id.as_bytes()))
            .map(|(hash, _)| *hash)
            .collect();
        for hash in drop_listings {
            if let Some(listing) = self.listings.remove(&hash) {
                let _ = fs::remove_file(self.listing_path(&hash));
                let _ = listing;
            }
        }
        Ok(())
    }

    fn load(&mut self) -> Result<(), String> {
        let path = self.dir.join("document.obsd");
        if let Ok(bytes) = fs::read(&path) {
            match SeedDocument::decode(&bytes) {
                Ok(document) => match document.verify_signer(&DOCUMENT_KEY) {
                    Ok(()) => {
                        eprintln!("ob-node: seed document sequence {}", document.sequence);
                        self.document = Some(document);
                    }
                    Err(_) => eprintln!("ob-node: seed document at {} is not signed by the shipped key", path.display()),
                },
                Err(err) => eprintln!("ob-node: seed document at {}: {err}", path.display()),
            }
        }
        if let Ok(text) = fs::read_to_string(self.dir.join("followed")) {
            for line in text.lines() {
                if let Some(id) = unhex32(line.trim()) {
                    self.followed.insert(id);
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("stores")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(store) = Store::decode(&bytes) else { continue };
                if store.verify().is_err() || !self.wanted(store.id.as_bytes()) {
                    continue;
                }
                self.stores.insert(*store.id.as_bytes(), store);
            }
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("listings")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(listing) = Listing::decode(&bytes) else { continue };
                if listing.verify().is_err() || !self.wanted(listing.store_id.as_bytes()) {
                    continue;
                }
                let Ok(hash) = listing.content_hash() else { continue };
                self.listings.insert(*hash.as_bytes(), listing);
            }
        }
        Ok(())
    }

    fn save_document(&self) -> Result<(), String> {
        let Some(document) = &self.document else { return Ok(()) };
        let bytes = document.encode().map_err(|err| err.to_string())?;
        atomic(&self.dir.join("document.obsd"), &bytes)
    }

    fn save_followed(&self) -> Result<(), String> {
        let mut text = String::new();
        for id in &self.followed {
            text.push_str(&hex(id));
            text.push('\n');
        }
        atomic(&self.dir.join("followed"), text.as_bytes())
    }

    fn save_store(&mut self, store: &Store) -> Result<(), String> {
        let bytes = store.encode().map_err(|err| err.to_string())?;
        atomic(&self.store_path(store.id.as_bytes()), &bytes)?;
        self.stores.insert(*store.id.as_bytes(), store.clone());
        Ok(())
    }

    fn save_listing(&mut self, hash: [u8; 32], listing: &Listing) -> Result<(), String> {
        let bytes = listing.encode().map_err(|err| err.to_string())?;
        atomic(&self.listing_path(&hash), &bytes)?;
        self.listings.insert(hash, listing.clone());
        Ok(())
    }

    fn store_path(&self, id: &[u8; 32]) -> PathBuf {
        self.dir.join("stores").join(hex(id))
    }

    fn listing_path(&self, hash: &[u8; 32]) -> PathBuf {
        self.dir.join("listings").join(hex(hash))
    }
}

impl Source for Replica {
    fn seed_bytes(&self) -> Result<Vec<u8>, String> {
        self.document.as_ref().ok_or_else(|| "no seed document".to_string())?.encode().map_err(|err| err.to_string())
    }

    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        match self.stores.get(id) {
            Some(store) => Ok(Some(store.encode().map_err(|err| err.to_string())?)),
            None => Ok(None),
        }
    }

    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        self.listings
            .values()
            .filter(|listing| listing.store_id.as_bytes() == id)
            .map(|listing| listing.encode().map_err(|err| err.to_string()))
            .collect()
    }
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    crate::stores::unhex32(text)
}

impl Node {
    /// Ask each compiled introduction. One that does not answer is skipped.
    pub(crate) fn directory_reply(&self, kind: u8, id: &[u8; 32]) -> Option<Vec<u8>> {
        let from_replica = self.seed_replica.lock().ok()?.answer(kind, id);
        match kind {
            tormail::QUERY_SEED => from_replica,
            tormail::QUERY_STORE => {
                if from_replica.is_some() {
                    return from_replica;
                }
                self.catalog().stores.iter().find(|store| store.id.as_bytes() == id)?.encode().ok()
            }
            tormail::QUERY_LISTINGS => {
                if from_replica.as_ref().is_some_and(|bytes| !bytes.is_empty()) {
                    return from_replica;
                }
                let catalog = self.catalog();
                let mut out = Vec::new();
                for listing in &catalog.listings {
                    if listing.store_id.as_bytes() != id {
                        continue;
                    }
                    let bytes = listing.encode().ok()?;
                    let len = u32::try_from(bytes.len()).ok()?;
                    out.extend_from_slice(&len.to_le_bytes());
                    out.extend_from_slice(&bytes);
                }
                if out.is_empty() { None } else { Some(out) }
            }
            _ => None,
        }
    }

    pub(crate) fn run_introductions(&self, stop: &AtomicBool) {
        let sources = self.introductions();
        while !stop.load(Ordering::Relaxed) {
            let refs: Vec<&dyn Source> = sources.iter().map(|source| source as &dyn Source).collect();
            if let Err(err) = self.pull_directory(&refs) {
                eprintln!("ob-node: introduction: {err}");
            }
            for _ in 0..60 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    }

    pub(crate) fn pull_directory(&self, sources: &[&dyn Source]) -> Result<(), String> {
        let own = self.own_store;
        let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        replica.pull(sources, &DOCUMENT_KEY)?;
        replica.apply(&mut self.catalog_mut(), own);
        Ok(())
    }

    pub(crate) fn follow_store(&self, id: [u8; 32]) -> Result<(), String> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).follow(id)
    }

    fn introductions(&self) -> Vec<Introduction> {
        let state_dir = self.data_dir.join("tor-client");
        crate::seeds::INTRODUCTIONS
            .iter()
            .map(|onion| Introduction { onion: (*onion).to_string(), state_dir: state_dir.clone() })
            .collect()
    }

    /// Sign a listing for this node's store and keep it in the catalog and
    /// the replica directory, so another peer can pull it after this process
    /// is gone from the network.
    pub(crate) fn publish_listing(&self, body: &Value) -> Result<Value, OrderError> {
        let secret = self.own_signing_key().ok_or(OrderError::Locked("locked"))?;
        let store = self.own_record().ok_or(OrderError::Invalid("This node has no store record yet."))?;
        let title = text_field(body, "title", 80)?;
        let category = text_field(body, "category", 40)?;
        let ship_from = text_field(body, "shipFrom", 40)?;
        let description = text_field(body, "description", 2000)?;
        let price = u64_field(body, "priceSats")?;
        let stock = u32_field(body, "stock")?;
        let shipping = u64_field(body, "shippingSats").unwrap_or(0);
        let free_over = u64_field(body, "freeOverSats").unwrap_or(0);
        let kind = match body.get("kind").and_then(Value::as_str).unwrap_or("physical") {
            "physical" => openbazaar_protocol::ListingKind::Physical,
            "digital" => openbazaar_protocol::ListingKind::Digital,
            _ => return Err(OrderError::Invalid("Kind is physical or digital.")),
        };
        let listing = Listing::sign(
            store.id,
            title,
            category,
            price,
            ship_from,
            description,
            store.name.clone(),
            stock,
            crate::own_store::today_yyyymmdd(),
            false,
            kind,
            "",
            shipping,
            free_over,
            openbazaar_protocol::ContentLabel::GENERAL,
            &secret,
        )
        .map_err(|err| OrderError::Failed(err.to_string()))?;
        let hash = listing.content_hash().map_err(|err| OrderError::Failed(err.to_string()))?;
        {
            let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.save_store(&store).map_err(OrderError::Failed)?;
            replica.save_listing(*hash.as_bytes(), &listing).map_err(OrderError::Failed)?;
        }
        self.catalog_mut().listings.push(listing.clone());
        listing_value(&self.catalog(), &listing).map_err(|err| OrderError::Failed(err.to_string()))
    }
}

/// Ask `onions` for the seed document and keep the highest sequence that verifies
/// under the shipped key. `dir` is one network's data directory (`…/testnet4`).
pub(crate) fn pull_onions(dir: &Path, onions: &[String]) -> Result<u64, String> {
    let mut replica = Replica::open(dir).map_err(|err| err.to_string())?;
    let state_dir = dir.join("tor-client");
    let sources: Vec<Introduction> = onions
        .iter()
        .map(|onion| Introduction { onion: onion.clone(), state_dir: state_dir.clone() })
        .collect();
    let reported: Vec<Report<'_>> = sources.iter().map(|source| Report { inner: source }).collect();
    let refs: Vec<&dyn Source> = reported.iter().map(|source| source as &dyn Source).collect();
    replica.pull(&refs, &DOCUMENT_KEY)?;
    replica
        .document
        .as_ref()
        .map(|document| document.sequence)
        .ok_or_else(|| "no introduction returned a document".into())
}

struct Report<'a> {
    inner: &'a Introduction,
}

impl Source for Report<'_> {
    fn seed_bytes(&self) -> Result<Vec<u8>, String> {
        self.inner.seed_bytes().inspect_err(|err| eprintln!("ob-node: introduction {}: {err}", self.inner.onion))
    }

    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        self.inner.store_bytes(id)
    }

    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        self.inner.listing_bytes(id)
    }
}

fn text_field(body: &Value, key: &str, max: usize) -> Result<String, OrderError> {
    let text = body.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    if text.is_empty() || text.len() > max {
        return Err(OrderError::Invalid("Enter a title, category, ship-from, and description within the limits."));
    }
    Ok(text)
}

fn u64_field(body: &Value, key: &str) -> Result<u64, OrderError> {
    body.get(key)
        .and_then(Value::as_u64)
        .ok_or(OrderError::Invalid("Enter a price in sats."))
}

fn u32_field(body: &Value, key: &str) -> Result<u32, OrderError> {
    let value = u64_field(body, key)?;
    u32::try_from(value).map_err(|_| OrderError::Invalid("Enter a stock count up to 100000."))
}

/// `POST /follow` with `{"storeId": "<64 hex>"}` and nothing else.
pub(crate) fn follow_response(node: &Node, client: &Client, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_error("400 Bad Request", "Add store takes a store key.", origin),
    };
    let Some(object) = value.as_object() else {
        return json_error("400 Bad Request", "Add store takes a store key.", origin);
    };
    if object.len() != 1 || !object.contains_key("storeId") {
        return json_error("400 Bad Request", "Add store takes a store key.", origin);
    }
    let Some(id) = object.get("storeId").and_then(Value::as_str).and_then(unhex32) else {
        return json_error("400 Bad Request", "The store key is 64 hex characters.", origin);
    };
    if let Err(err) = node.follow_store(id) {
        eprintln!("ob-node: follow: {err}");
        return json_error("500 Internal Server Error", "The store key could not be saved.", origin);
    }
    json_body("200 OK", &json!({ "storeId": hex(&id) }), origin)
}

pub(crate) fn listing_publish_response(
    node: &Node,
    client: &Client,
    head: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_error("400 Bad Request", "The listing could not be read.", origin),
    };
    match node.publish_listing(&value) {
        Ok(listing) => json_body("200 OK", &listing, origin),
        Err(OrderError::Locked(_)) => json_error("423 Locked", "Unlock the wallet to sign a listing.", origin),
        Err(OrderError::Invalid(message)) => json_error("400 Bad Request", message, origin),
        Err(OrderError::Failed(err)) => {
            eprintln!("ob-node: listing: {err}");
            json_error("400 Bad Request", "The listing was not signed.", origin)
        }
        Err(OrderError::UnknownListing) => json_error("400 Bad Request", "The listing was not signed.", origin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;
    use openbazaar_protocol::{fixture_catalog, secret_from_label, xonly, ContentLabel, ListingKind, SeedDocument};

    struct Held {
        document: Vec<u8>,
        store: Option<Vec<u8>>,
        listings: Vec<Vec<u8>>,
        up: bool,
    }

    impl Source for Held {
        fn seed_bytes(&self) -> Result<Vec<u8>, String> {
            if self.up {
                Ok(self.document.clone())
            } else {
                Err("stopped".into())
            }
        }
        fn store_bytes(&self, _: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
            if self.up {
                Ok(self.store.clone())
            } else {
                Err("stopped".into())
            }
        }
        fn listing_bytes(&self, _: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
            if self.up {
                Ok(self.listings.clone())
            } else {
                Err("stopped".into())
            }
        }
    }

    fn onion(label: char) -> String {
        format!("{}.onion", label.to_string().repeat(56))
    }

    #[test]
    fn one_stopped_introduction_still_yields_the_higher_document_and_the_listing_survives() {
        let catalog = fixture_catalog();
        let store = catalog.stores[0].clone();
        let listings: Vec<_> = catalog
            .listings
            .iter()
            .filter(|listing| listing.store_id == store.id)
            .cloned()
            .collect();
        let secret = secret_from_label("directory-gate");
        let key = xonly(&secret);
        let older = SeedDocument::sign(1, vec![onion('a')], vec![*store.id.as_bytes()], None, &secret).unwrap();
        let newer = SeedDocument::sign(
            5,
            vec![onion('b'), onion('a')],
            vec![*store.id.as_bytes()],
            None,
            &secret,
        )
        .unwrap();
        let low = Held {
            document: older.encode().unwrap(),
            store: None,
            listings: vec![],
            up: true,
        };
        let high = Held {
            document: newer.encode().unwrap(),
            store: Some(store.encode().unwrap()),
            listings: listings.iter().map(|listing| listing.encode().unwrap()).collect(),
            up: true,
        };
        let stopped = Held {
            document: newer.encode().unwrap(),
            store: Some(store.encode().unwrap()),
            listings: high.listings.clone(),
            up: false,
        };

        let dir = TempDir::new();
        let mut peer = Replica::open(&dir.0).unwrap();
        peer.pull(&[&stopped, &high], &key).unwrap();
        assert_eq!(peer.document.as_ref().unwrap().sequence, 5);
        assert_eq!(peer.stores.len(), 1);
        assert!(!peer.listings.is_empty());

        peer.pull(&[&low, &stopped], &key).unwrap();
        assert_eq!(peer.document.as_ref().unwrap().sequence, 5, "an older document and a stopped host do not replace it");

        drop(high);
        drop(stopped);
        drop(peer);
        // The document file is signed by the test key, not DOCUMENT_KEY, so a
        // reload ignores it. Following the store is what keeps the files wanted.
        let mut kept = Replica::open(&dir.0).unwrap();
        kept.follow(*store.id.as_bytes()).unwrap();
        let reloaded = Replica::open(&dir.0).unwrap();
        let mut fresh = Catalog { stores: vec![], moderator: catalog.moderator.clone(), listings: vec![] };
        reloaded.apply(&mut fresh, StoreId::from_bytes([9u8; 32]));
        assert!(fresh.listings.iter().any(|listing| listing.store_id == store.id));
        assert!(fresh.stores.iter().any(|item| item.id == store.id));
    }

    #[test]
    fn a_published_listing_verifies_under_the_store_key() {
        let _ = (ContentLabel::GENERAL, ListingKind::Physical);
        let secret = secret_from_label("own-store-listing");
        let id = StoreId::from_bytes(xonly(&secret));
        let listing = Listing::sign(
            id,
            "Cedar spoon",
            "Home",
            2_400,
            "Maine",
            "A small cedar spoon for the kitchen.",
            "My store",
            3,
            20261109,
            false,
            ListingKind::Physical,
            "",
            300,
            0,
            ContentLabel::GENERAL,
            &secret,
        )
        .unwrap();
        listing.verify().unwrap();
        assert_eq!(listing.store_id, id);
    }

    #[test]
    fn the_node_signs_a_posted_listing_and_add_store_takes_only_a_key() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let body = r#"{"title":"Cedar spoon","category":"Home","shipFrom":"Maine","description":"A small cedar spoon.","priceSats":2400,"stock":3}"#;
        let (status, value) = harness::request(&node, "POST", "/listings", body);
        assert!(status.contains("200"), "{status} {value}");
        {
            let catalog = node.catalog();
            let listing = catalog.listings.iter().find(|item| item.title == "Cedar spoon").unwrap();
            listing.verify().unwrap();
            assert_eq!(listing.store_id, node.own_store);
        }

        let extra = format!(r#"{{"storeId":"{}","onion":"not-a-destination"}}"#, "ab".repeat(32));
        let (status, _) = harness::request(&node, "POST", "/follow", &extra);
        assert!(status.contains("400"), "{status}");
        let (status, value) = harness::request(&node, "POST", "/follow", &format!(r#"{{"storeId":"{}"}}"#, "cd".repeat(32)));
        assert!(status.contains("200"), "{status} {value}");
    }

    #[test]
    fn two_nodes_each_keep_the_others_listing_after_that_node_is_gone() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        struct Peer<'a>(&'a crate::Node);
        impl Source for Peer<'_> {
            fn seed_bytes(&self) -> Result<Vec<u8>, String> {
                Err("this peer is not an introduction".into())
            }
            fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
                let replica = self.0.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
                Source::store_bytes(&*replica, id)
            }
            fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
                let replica = self.0.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
                Source::listing_bytes(&*replica, id)
            }
        }

        let a_dir = TempDir::new();
        let b_dir = TempDir::new();
        let chain = harness::MockChain::new();
        let a = harness::open_node(&a_dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let b = harness::open_node(&b_dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let a_store = *a.own_store.as_bytes();
        let b_store = *b.own_store.as_bytes();
        let cedar = r#"{"title":"Cedar spoon","category":"Home","shipFrom":"Maine","description":"A small cedar spoon.","priceSats":2400,"stock":3}"#;
        let maple = r#"{"title":"Maple spoon","category":"Home","shipFrom":"Vermont","description":"A small maple spoon.","priceSats":2600,"stock":2}"#;
        let (status, value) = harness::request(&a, "POST", "/listings", cedar);
        assert!(status.contains("200"), "{status} {value}");
        let (status, value) = harness::request(&b, "POST", "/listings", maple);
        assert!(status.contains("200"), "{status} {value}");
        a.follow_store(b_store).unwrap();
        b.follow_store(a_store).unwrap();
        a.pull_directory(&[&Peer(&b)]).unwrap();
        b.pull_directory(&[&Peer(&a)]).unwrap();
        {
            let catalog = a.catalog();
            let listing = catalog.listings.iter().find(|item| item.title == "Maple spoon").unwrap();
            assert_eq!(listing.store_id.as_bytes(), &b_store);
            listing.verify().unwrap();
        }
        {
            let catalog = b.catalog();
            let listing = catalog.listings.iter().find(|item| item.title == "Cedar spoon").unwrap();
            assert_eq!(listing.store_id.as_bytes(), &a_store);
            listing.verify().unwrap();
        }
        drop(a);
        drop(b);
        let kept = harness::open_node(&b_dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let catalog = kept.catalog();
        let listing = catalog.listings.iter().find(|item| item.title == "Cedar spoon").unwrap();
        assert_eq!(listing.store_id.as_bytes(), &a_store);
        listing.verify().unwrap();
    }

    #[test]
    fn the_shipped_introductions_are_v3_onions() {
        for onion in crate::seeds::INTRODUCTIONS {
            tormail::OnionAddress::parse(onion).unwrap();
        }
    }
}
