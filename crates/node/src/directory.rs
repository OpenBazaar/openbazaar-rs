//! Directory replication.
//!
//! A peer asks up to three introductions for the head and keeps the highest
//! sequence that verifies under the shipped key. Stores join with an
//! admission, not with a new sequence. Store records are wanted for an
//! admitted or followed id. Listing bodies are wanted for the own store,
//! followed ids, and the 50 stores last opened here. A source that is down
//! is skipped. A bad byte does not delete the last good record.
//!
//! Nothing here accepts a client-supplied onion. Add-store takes a store key.
//! The record is kept only when its signature matches that key.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use openbazaar_protocol::{Admission, Catalog, Listing, SeedDocument, Store, StoreId, ADMISSION_LEN};
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

    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, tormail::QUERY_ADMISSIONS, *cursor)
    }
}

fn submit_record(state_dir: &Path, onion: &str, kind: u8, body: &[u8]) -> Result<bool, String> {
    let onion = tormail::OnionAddress::parse(onion).map_err(|err| err.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(async {
            let client = tormail::open_client(state_dir)?;
            tokio::time::timeout(
                Duration::from_secs(30),
                tormail::submit_directory(&client, &onion, kind, body),
            )
            .await
            .map_err(|_| tormail::Error::Transport("introduction timed out"))?
        })
        .map_err(|err| err.to_string())
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
    /// One page of admissions with store ids greater than `cursor`.
    /// `Ok(None)` means this source has none. An error means it did not answer.
    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        let _ = cursor;
        Ok(None)
    }
}

const PASS_ATTEMPTS: usize = 3;
const OPENED_CAP: usize = 50;
fn page_cap() -> usize {
    tormail::MAX_QUERY_LEN / ADMISSION_LEN
}

pub(crate) struct Replica {
    dir: PathBuf,
    document: Option<SeedDocument>,
    followed: BTreeSet<[u8; 32]>,
    /// Store keys submitted before admissions existed. Not a way to join.
    inbox: BTreeSet<[u8; 32]>,
    admissions: BTreeMap<[u8; 32], Admission>,
    /// Next admission page starts after this store id. Wrapped to zero after a short page.
    cursor: [u8; 32],
    /// A short admission page has been seen, so later passes take one page.
    synced: bool,
    /// Store ids opened on this node, oldest first. The last 50 want listings.
    opened: Vec<[u8; 32]>,
    stores: BTreeMap<[u8; 32], Store>,
    listings: BTreeMap<[u8; 32], Listing>,
    /// Store ids whose admission passed the fee check when the chain was last asked.
    live: BTreeSet<[u8; 32]>,
}

/// Where a pull keeps what it fetched. A shared replica is locked for each
/// step and free while a source is asked, so a slow introduction does not
/// hold up a request that reads the replica.
pub(crate) trait Held {
    fn with<T>(&self, step: impl FnOnce(&mut Replica) -> T) -> T;
}

impl Held for Mutex<Replica> {
    fn with<T>(&self, step: impl FnOnce(&mut Replica) -> T) -> T {
        step(&mut self.lock().unwrap_or_else(|err| err.into_inner()))
    }
}

impl Held for RefCell<&mut Replica> {
    fn with<T>(&self, step: impl FnOnce(&mut Replica) -> T) -> T {
        step(&mut self.borrow_mut())
    }
}

impl Replica {
    pub(crate) fn open(dir: &Path) -> Result<Self, Error> {
        let dir = dir.join("directory");
        fs::create_dir_all(dir.join("stores")).map_err(|err| Error::Detail(err.to_string()))?;
        fs::create_dir_all(dir.join("listings")).map_err(|err| Error::Detail(err.to_string()))?;
        fs::create_dir_all(dir.join("admissions")).map_err(|err| Error::Detail(err.to_string()))?;
        let mut replica = Self {
            dir,
            document: None,
            followed: BTreeSet::new(),
            inbox: BTreeSet::new(),
            admissions: BTreeMap::new(),
            cursor: [0u8; 32],
            synced: false,
            opened: Vec::new(),
            stores: BTreeMap::new(),
            listings: BTreeMap::new(),
            live: BTreeSet::new(),
        };
        replica.load().map_err(|err| Error::Detail(err))?;
        Ok(replica)
    }

    pub(crate) fn head(&self) -> Option<SeedDocument> {
        self.document.clone()
    }

    /// Hosts to dial. A saved head supplies its own list. The compiled onions
    /// are only the bootstrap for an empty `document.obsd`.
    pub(crate) fn introduction_hosts(&self) -> Vec<String> {
        match &self.document {
            Some(document) if !document.introductions.is_empty() => {
                document.introductions.iter().take(openbazaar_protocol::MAX_INTRODUCTIONS).cloned().collect()
            }
            _ => crate::seeds::INTRODUCTIONS.iter().map(|onion| (*onion).to_string()).collect(),
        }
    }

    /// Ask up to three sources for a newer head, then one or more admission
    /// pages, then store records for every admitted or followed id that has
    /// no record yet. An id admitted on an earlier pass is asked again until
    /// its record is kept. Listings are fetched only for the own store,
    /// follows, and recently opened stores, and only when this node does not
    /// already hold one.
    pub(crate) fn pull(&mut self, sources: &[&dyn Source], key: &[u8; 32], own: [u8; 32]) -> Result<(), String> {
        pull_into(&RefCell::new(self), sources, key, own)
    }

    /// Keep the highest sequence among the held head and `fetched`.
    fn keep_head(&mut self, fetched: Vec<SeedDocument>) -> Result<(), String> {
        let mut best = self.document.clone();
        for document in fetched {
            let newer = best.as_ref().map(|held| document.sequence > held.sequence).unwrap_or(true);
            if newer {
                best = Some(document);
            }
        }
        if let Some(document) = best {
            if self.document.as_ref().map(|held| held.sequence) != Some(document.sequence) {
                self.document = Some(document);
                self.save_document()?;
            }
        }
        Ok(())
    }

    /// Keep one admission page. `true` asks for the page after it.
    fn keep_admission_page(&mut self, blob: &[u8]) -> Result<bool, String> {
        if blob.len() % ADMISSION_LEN != 0 {
            return Ok(false);
        }
        let count = blob.len() / ADMISSION_LEN;
        let mut last = self.cursor;
        for chunk in blob.chunks(ADMISSION_LEN) {
            if let Some(id) = Admission::peek_store_id(chunk) {
                if id > last {
                    last = id;
                }
            }
            if let Ok(admission) = Admission::decode(chunk) {
                let id = admission.store_id;
                self.admissions.insert(id, admission);
                self.save_admission_file(id)?;
            }
        }
        if count < page_cap() {
            self.synced = true;
            self.cursor = [0u8; 32];
            self.save_cursor()?;
            return Ok(false);
        }
        self.cursor = last;
        self.save_cursor()?;
        Ok(!self.synced)
    }

    /// Every admitted or followed id with no store file. A missing record stays
    /// wanted on the next pass, including an id already in the admission map.
    fn wanted_stores(&self) -> BTreeSet<[u8; 32]> {
        self.admissions.keys().chain(self.followed.iter()).filter(|id| !self.stores.contains_key(*id)).copied().collect()
    }

    pub(crate) fn admissions(&self) -> Vec<Admission> {
        self.admissions.values().cloned().collect()
    }

    /// Whether `admission` passed the fee check when the chain was last asked.
    pub(crate) fn is_live(&self, admission: &Admission) -> bool {
        self.live.contains(&admission.store_id)
    }

    pub(crate) fn set_live(&mut self, live: BTreeSet<[u8; 32]>) {
        self.live = live;
    }

    fn holds_listing(&self, id: &[u8; 32]) -> bool {
        self.listings.values().any(|listing| listing.store_id.as_bytes() == id)
    }

    fn listing_ids(&self, own: [u8; 32]) -> Vec<[u8; 32]> {
        let mut ids = Vec::new();
        if own != [0u8; 32] {
            ids.push(own);
        }
        for id in &self.followed {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        for id in self.opened.iter().rev().take(OPENED_CAP) {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        ids
    }

    pub(crate) fn answer(&self, kind: u8, id: &[u8; 32]) -> Option<Vec<u8>> {
        match kind {
            tormail::QUERY_SEED => self.document.as_ref()?.encode().ok(),
            tormail::QUERY_STORE => self.stores.get(id)?.encode().ok(),
            tormail::QUERY_ADMISSIONS => Some(self.admission_page(id)),
            tormail::QUERY_INBOX => {
                let mut out = Vec::new();
                for id in &self.inbox {
                    out.extend_from_slice(id);
                }
                Some(out)
            }
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

    fn admission_page(&self, cursor: &[u8; 32]) -> Vec<u8> {
        let mut out = Vec::new();
        for admission in self.admissions.values().filter(|admission| admission.store_id > *cursor).take(page_cap()) {
            if let Ok(bytes) = admission.encode() {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }

    pub(crate) fn follow(&mut self, id: [u8; 32]) -> Result<(), String> {
        if self.followed.insert(id) {
            self.save_followed()?;
        }
        Ok(())
    }

    pub(crate) fn unfollow(&mut self, id: [u8; 32]) -> Result<(), String> {
        if self.followed.remove(&id) {
            self.save_followed()?;
        }
        Ok(())
    }

    pub(crate) fn note_opened(&mut self, id: [u8; 32]) -> Result<(), String> {
        self.opened.retain(|open| open != &id);
        self.opened.push(id);
        if self.opened.len() > OPENED_CAP {
            let drop = self.opened.len() - OPENED_CAP;
            self.opened.drain(..drop);
        }
        self.save_opened()
    }

    /// Keep a verified store. A byte that does not verify leaves the previous record.
    pub(crate) fn accept_store(&mut self, bytes: &[u8]) -> Result<(), String> {
        let store = Store::decode(bytes).map_err(|err| err.to_string())?;
        store.verify().map_err(|err| err.to_string())?;
        self.save_store(&store)
    }

    /// Keep a verified listing. A changed byte, or a listing with no store record, is refused.
    pub(crate) fn accept_listing(&mut self, bytes: &[u8]) -> Result<(), String> {
        let listing = Listing::decode(bytes).map_err(|err| err.to_string())?;
        listing.verify().map_err(|err| err.to_string())?;
        let id = *listing.store_id.as_bytes();
        if !self.stores.contains_key(&id) {
            return Err("that listing has no store record".into());
        }
        let hash = listing.content_hash().map_err(|err| err.to_string())?;
        self.save_listing(*hash.as_bytes(), &listing)
    }

    /// Keep a verified admission. A flipped byte does not replace a good one.
    pub(crate) fn accept_admission(&mut self, bytes: &[u8]) -> Result<(), String> {
        let admission = Admission::decode(bytes).map_err(|err| err.to_string())?;
        let id = admission.store_id;
        self.admissions.insert(id, admission);
        self.save_admission_file(id)
    }

    /// Keep listings that verify. A listing that does not verify is skipped.
    pub(crate) fn keep_listings(&mut self, id: &[u8; 32], lists: &[Vec<u8>]) -> Result<(), String> {
        for bytes in lists {
            let Ok(listing) = Listing::decode(bytes) else { continue };
            if listing.verify().is_err() || listing.store_id.as_bytes() != id {
                continue;
            }
            let Ok(hash) = listing.content_hash() else { continue };
            self.save_listing(*hash.as_bytes(), &listing)?;
        }
        Ok(())
    }

    /// Keep a higher sequence signed by `key`.
    pub(crate) fn accept_document(&mut self, bytes: &[u8], key: &[u8; 32]) -> Result<u64, String> {
        let document = SeedDocument::decode(bytes).map_err(|err| err.to_string())?;
        document.verify_signer(key).map_err(|err| err.to_string())?;
        if self.document.as_ref().is_some_and(|held| document.sequence <= held.sequence) {
            return Err("an older sequence does not replace the document".into());
        }
        let sequence = document.sequence;
        self.document = Some(document);
        self.save_document()?;
        Ok(sequence)
    }

    /// The signed name is shown when this node follows the id, or the admission
    /// still passes `live`, and a verified store record is held. A missing
    /// record is not a shop. The own id is shown from this node's catalog, not here.
    pub(crate) fn show_store(&self, id: &[u8; 32], live: &dyn Fn(&Admission) -> bool) -> bool {
        if !self.stores.contains_key(id) {
            return false;
        }
        self.followed.contains(id) || self.admissions.get(id).is_some_and(live)
    }

    pub(crate) fn listing_wanted(&self, id: &[u8; 32], own: &[u8; 32]) -> bool {
        id == own || self.followed.contains(id) || self.opened.iter().rev().take(OPENED_CAP).any(|open| open == id)
    }

    /// Names whose store record matches, then titles whose listing is held.
    pub(crate) fn search(&self, query: &str, own: &[u8; 32], live: &dyn Fn(&Admission) -> bool) -> (Vec<String>, Vec<String>) {
        let needle = query.trim().to_lowercase();
        let mut names = Vec::new();
        let mut titles = Vec::new();
        if needle.is_empty() {
            return (names, titles);
        }
        for store in self.stores.values() {
            if !self.show_store(store.id.as_bytes(), live) {
                continue;
            }
            let profile = store.details.as_ref().map(|details| &details.profile);
            let about = profile.map(|profile| profile.about.as_str()).unwrap_or("");
            let tagline = profile.map(|profile| profile.tagline.as_str()).unwrap_or("");
            if [store.name.as_str(), store.region.as_str(), tagline, about].iter().any(|field| field.to_lowercase().contains(&needle)) {
                names.push(store.name.clone());
            }
        }
        for listing in self.listings.values() {
            let id = listing.store_id.as_bytes();
            if !self.show_store(id, live) || !self.listing_wanted(id, own) {
                continue;
            }
            if listing.title.to_lowercase().contains(&needle) || listing.category.to_lowercase().contains(&needle) {
                titles.push(listing.title.clone());
            }
        }
        (names, titles)
    }

    /// Put replicated records into `catalog`. `keep` is the own store and demo stores.
    pub(crate) fn apply(
        &self,
        catalog: &mut Catalog,
        own: StoreId,
        keep: impl Fn(&Store) -> bool,
        live: impl Fn(&Admission) -> bool,
    ) {
        let local: BTreeSet<_> = catalog.stores.iter().filter(|store| keep(store)).map(|store| store.id).collect();
        let show = |id: &[u8; 32]| self.show_store(id, &live);
        catalog.stores.retain(|store| local.contains(&store.id) || show(store.id.as_bytes()));
        catalog.listings.retain(|listing| {
            local.contains(&listing.store_id)
                || (show(listing.store_id.as_bytes()) && self.listing_wanted(listing.store_id.as_bytes(), own.as_bytes()))
        });
        for store in self.stores.values() {
            if local.contains(&store.id) || !show(store.id.as_bytes()) {
                continue;
            }
            match catalog.stores.iter_mut().find(|have| have.id == store.id) {
                Some(have) => *have = store.clone(),
                None => catalog.stores.push(store.clone()),
            }
        }
        for listing in self.listings.values() {
            if !show(listing.store_id.as_bytes()) || !self.listing_wanted(listing.store_id.as_bytes(), own.as_bytes()) {
                continue;
            }
            let hash = listing.content_hash().ok();
            let present = catalog.listings.iter().any(|have| have.content_hash().ok() == hash);
            if !present {
                catalog.listings.push(listing.clone());
            }
        }
    }

    fn load(&mut self) -> Result<(), String> {
        let path = self.dir.join("document.obsd");
        if let Ok(bytes) = fs::read(&path) {
            match SeedDocument::decode(&bytes) {
                Ok(document) => match document.verify_signer(&crate::seeds::DOCUMENT_KEY) {
                    Ok(()) => {
                        eprintln!("ob-node: directory head sequence {}", document.sequence);
                        self.document = Some(document);
                    }
                    Err(_) => eprintln!("ob-node: directory head at {} is not signed by the shipped key", path.display()),
                },
                Err(err) => eprintln!("ob-node: directory head at {}: {err}", path.display()),
            }
        }
        if let Ok(text) = fs::read_to_string(self.dir.join("followed")) {
            for line in text.lines() {
                if let Some(id) = unhex32(line.trim()) {
                    self.followed.insert(id);
                }
            }
        }
        if let Ok(text) = fs::read_to_string(self.dir.join("inbox")) {
            for line in text.lines() {
                if let Some(id) = unhex32(line.trim()) {
                    self.inbox.insert(id);
                }
            }
        }
        if let Ok(text) = fs::read_to_string(self.dir.join("opened")) {
            for line in text.lines() {
                if let Some(id) = unhex32(line.trim()) {
                    self.opened.push(id);
                }
            }
        }
        if let Ok(text) = fs::read_to_string(self.dir.join("cursor")) {
            let mut lines = text.lines();
            if let Some(id) = lines.next().and_then(unhex32) {
                self.cursor = id;
            }
            self.synced = lines.next().is_some_and(|line| line.trim() == "synced");
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("admissions")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(admission) = Admission::decode(&bytes) else { continue };
                self.admissions.insert(admission.store_id, admission);
            }
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("stores")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(store) = Store::decode(&bytes) else { continue };
                if store.verify().is_err() {
                    continue;
                }
                self.stores.insert(*store.id.as_bytes(), store);
            }
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("listings")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(listing) = Listing::decode(&bytes) else { continue };
                if listing.verify().is_err() {
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

    fn save_opened(&self) -> Result<(), String> {
        let mut text = String::new();
        for id in &self.opened {
            text.push_str(&hex(id));
            text.push('\n');
        }
        atomic(&self.dir.join("opened"), text.as_bytes())
    }

    fn save_cursor(&self) -> Result<(), String> {
        let mut text = hex(&self.cursor);
        text.push('\n');
        if self.synced {
            text.push_str("synced\n");
        }
        atomic(&self.dir.join("cursor"), text.as_bytes())
    }

    fn save_admission_file(&self, id: [u8; 32]) -> Result<(), String> {
        let Some(admission) = self.admissions.get(&id) else { return Ok(()) };
        let bytes = admission.encode().map_err(|err| err.to_string())?;
        atomic(&self.dir.join("admissions").join(hex(&id)), &bytes)
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

    pub(crate) fn read_outbox(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.join("outbox").join(name)).ok()
    }

    pub(crate) fn write_outbox(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let outbox = self.dir.join("outbox");
        fs::create_dir_all(&outbox).map_err(|err| err.to_string())?;
        atomic(&outbox.join(name), bytes)
    }

    pub(crate) fn queue_admission(&self, bytes: &[u8]) -> Result<(), String> {
        self.write_outbox("admission", bytes)
    }

    pub(crate) fn queue_store(&self, store: &Store) -> Result<(), String> {
        self.write_outbox("store", &store.encode().map_err(|err| err.to_string())?)
    }

    /// `directory/outbox/store` and `directory/outbox/listing-{hash}`, the bytes
    /// [`Node::flush_outbox`] submits. A later publish overwrites them. Flush
    /// leaves the files in place and submits the store again. A listing is
    /// submitted once.
    fn queue_outbox(&self, store: &Store, listing: &Listing) -> Result<(), String> {
        let hash = listing.content_hash().map_err(|err| err.to_string())?;
        self.queue_store(store)?;
        self.write_outbox(&format!("listing-{}", hex(hash.as_bytes())), &listing.encode().map_err(|err| err.to_string())?)
    }

    fn store_path(&self, id: &[u8; 32]) -> PathBuf {
        self.dir.join("stores").join(hex(id))
    }

    fn listing_path(&self, hash: &[u8; 32]) -> PathBuf {
        self.dir.join("listings").join(hex(hash))
    }

    pub(crate) fn outbox_listings(&self) -> Vec<(String, Vec<u8>)> {
        let Ok(entries) = fs::read_dir(self.dir.join("outbox")) else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("listing-") || name.ends_with(".sent") {
                    return None;
                }
                let bytes = fs::read(entry.path()).ok()?;
                Some((name, bytes))
            })
            .collect()
    }

    pub(crate) fn listing_was_sent(&self, name: &str) -> bool {
        self.dir.join("outbox").join(format!("{name}.sent")).exists()
    }

    pub(crate) fn mark_listing_sent(&self, name: &str) -> Result<(), String> {
        atomic(&self.dir.join("outbox").join(format!("{name}.sent")), b"1")
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

    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.admission_page(cursor)))
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

/// `http://` plus a v3 onion host. Anything else is not a sale destination.
/// One pull into `held`. Every fetch runs with the replica free.
fn pull_into(held: &impl Held, sources: &[&dyn Source], key: &[u8; 32], own: [u8; 32]) -> Result<(), String> {
    let sources = &sources[..sources.len().min(PASS_ATTEMPTS)];
    pull_head(held, sources, key)?;
    pull_admissions(held, sources)?;
    pull_stores(held, sources)?;
    pull_wanted_listings(held, sources, own)
}

fn pull_head(held: &impl Held, sources: &[&dyn Source], key: &[u8; 32]) -> Result<(), String> {
    let fetched = sources
        .iter()
        .filter_map(|source| source.seed_bytes().ok())
        .filter_map(|bytes| SeedDocument::decode(&bytes).ok())
        .filter(|document| document.verify_signer(key).is_ok())
        .collect();
    held.with(|replica| replica.keep_head(fetched))
}

fn pull_admissions(held: &impl Held, sources: &[&dyn Source]) -> Result<(), String> {
    let pages = if held.with(|replica| replica.synced) { 1 } else { 8 };
    for _ in 0..pages {
        let cursor = held.with(|replica| replica.cursor);
        let page = sources.iter().find_map(|source| source.admission_bytes(&cursor).ok().flatten());
        let Some(blob) = page else { break };
        if !held.with(|replica| replica.keep_admission_page(&blob))? {
            break;
        }
    }
    Ok(())
}

fn pull_stores(held: &impl Held, sources: &[&dyn Source]) -> Result<(), String> {
    for id in held.with(|replica| replica.wanted_stores()) {
        for source in sources {
            let Ok(Some(bytes)) = source.store_bytes(&id) else { continue };
            if let Ok(store) = Store::decode(&bytes) {
                if store.verify().is_ok() && store.id.as_bytes() == &id {
                    held.with(|replica| replica.save_store(&store))?;
                    break;
                }
            }
        }
    }
    Ok(())
}

fn pull_wanted_listings(held: &impl Held, sources: &[&dyn Source], own: [u8; 32]) -> Result<(), String> {
    for id in held.with(|replica| replica.listing_ids(own)) {
        if held.with(|replica| replica.holds_listing(&id)) {
            continue;
        }
        for source in sources {
            let Ok(lists) = source.listing_bytes(&id) else { continue };
            let kept = held.with(|replica| replica.keep_listings(&id, &lists).map(|()| replica.holds_listing(&id)))?;
            if kept {
                break;
            }
        }
    }
    Ok(())
}

pub(crate) fn sale_url(onion: &str) -> Option<String> {
    let onion = tormail::OnionAddress::parse(onion).ok()?;
    Some(format!("http://{onion}"))
}

/// Listings from the store onion, or from the fallback when that dial fails.
pub(crate) fn listings_after_dial(primary: Result<Vec<Vec<u8>>, String>, fallback: Result<Vec<Vec<u8>>, String>) -> Vec<Vec<u8>> {
    match primary {
        Ok(lists) => lists,
        Err(_) => fallback.unwrap_or_default(),
    }
}

fn names_destination(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            let folded: String = key.chars().filter(|ch| ch.is_ascii_alphanumeric()).flat_map(|ch| ch.to_lowercase()).collect();
            matches!(folded.as_str(), "onion" | "url" | "uri" | "destination" | "peer") || names_destination(child)
        }),
        Value::Array(items) => items.iter().any(names_destination),
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            lower.contains("://") || lower.contains(".onion")
        }
        _ => false,
    }
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
            tormail::QUERY_ADMISSIONS | tormail::QUERY_INBOX => from_replica,
            _ => None,
        }
    }

    /// Keep a signed store, listing, admission, or a higher head.
    pub(crate) fn directory_submit(&self, kind: u8, body: &[u8]) -> bool {
        let accepted = {
            let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            match kind {
                tormail::SUBMIT_STORE => replica.accept_store(body).map(|_| ()),
                tormail::SUBMIT_LISTING => replica.accept_listing(body),
                tormail::SUBMIT_DOCUMENT => replica.accept_document(body, &DOCUMENT_KEY).map(|_| ()),
                tormail::SUBMIT_ADMISSION => replica.accept_admission(body).map(|_| ()),
                _ => Err("unknown directory submission".into()),
            }
        };
        if accepted.is_ok() {
            self.recheck_admissions();
            self.refresh_market();
        }
        accepted.is_ok()
    }

    /// Ask the chain which admissions still pass the fee check. The replica
    /// is free while the chain answers. A request from the shop does not
    /// call this: it reads the answer kept here.
    fn recheck_admissions(&self) {
        let (head, admissions) = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            (replica.head(), replica.admissions())
        };
        let live = crate::admission::live_admissions(self.chain.as_ref(), self.network, head.as_ref(), &admissions);
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).set_live(live);
    }

    fn refresh_market(&self) {
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let own = self.own_store;
        let mut catalog = self.catalog.write().unwrap_or_else(|err| err.into_inner());
        replica.apply(
            &mut catalog,
            own,
            |store| store.id == own || crate::demo_secret(store).is_some(),
            |admission| replica.is_live(admission),
        );
    }

    /// Stores this node shows, by key and name. No onion and no URL.
    /// A followed id with no verified record is not shown as a shop.
    pub(crate) fn directory_value(&self) -> Value {
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let live = |admission: &Admission| replica.is_live(admission);
        let mut stores = Vec::new();
        for store in replica.stores.values() {
            let own = store.id == self.own_store;
            if !own && !replica.show_store(store.id.as_bytes(), &live) {
                continue;
            }
            let listings = replica.listings.values().filter(|listing| listing.store_id == store.id && replica.listing_wanted(store.id.as_bytes(), self.own_store.as_bytes())).count();
            stores.push(json!({
                "storeId": hex(store.id.as_bytes()),
                "name": store.name,
                "listings": listings,
                "added": replica.followed.contains(store.id.as_bytes()),
            }));
        }
        json!({ "stores": stores })
    }

    pub(crate) fn unfollow_store(&self, id: [u8; 32]) -> Result<(), String> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).unfollow(id)?;
        self.refresh_market();
        Ok(())
    }

    /// The sale onion inside a replicated store record, when this node does not play that store.
    pub(crate) fn directory_sale_url(&self, id: &StoreId) -> Option<String> {
        if *id == self.own_store {
            return None;
        }
        let catalog = self.catalog();
        let store = catalog.stores.iter().find(|store| store.id == *id)?;
        if crate::demo_secret(store).is_some() || self.store_wallets.contains_key(&store.name) {
            return None;
        }
        let onion = store.details.as_ref().map(|details| details.onion.as_str()).unwrap_or("");
        sale_url(onion)
    }

    pub(crate) fn run_introductions(&self, stop: &AtomicBool) {
        let mut origin = 0usize;
        while !stop.load(Ordering::Relaxed) {
            let sources = self.pass_introductions(origin);
            origin = origin.saturating_add(PASS_ATTEMPTS);
            self.flush_outbox(&sources);
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

    fn flush_outbox(&self, sources: &[Introduction]) {
        if let Err(err) = self.publish_admission() {
            eprintln!("ob-node: directory fee: {err}");
        }
        let (store, admission, listings) = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            (replica.read_outbox("store"), replica.read_outbox("admission"), replica.outbox_listings())
        };
        for source in sources {
            if let Some(bytes) = &store {
                let _ = submit_record(&source.state_dir, &source.onion, tormail::SUBMIT_STORE, bytes);
            }
            if let Some(bytes) = &admission {
                let _ = submit_record(&source.state_dir, &source.onion, tormail::SUBMIT_ADMISSION, bytes);
            }
        }
        for (name, bytes) in &listings {
            let already = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).listing_was_sent(name);
            if already {
                continue;
            }
            let mut kept = false;
            for source in sources {
                if submit_record(&source.state_dir, &source.onion, tormail::SUBMIT_LISTING, bytes).ok() == Some(true) {
                    kept = true;
                    break;
                }
            }
            if kept {
                let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
                let _ = replica.mark_listing_sent(name);
            }
        }
    }

    pub(crate) fn pull_directory(&self, sources: &[&dyn Source]) -> Result<(), String> {
        let own = *self.own_store.as_bytes();
        pull_into(&self.seed_replica, sources, &DOCUMENT_KEY, own)?;
        self.recheck_admissions();
        self.refresh_market();
        Ok(())
    }

    pub(crate) fn follow_store(&self, id: [u8; 32]) -> Result<(), String> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).follow(id)
    }

    /// Open one store: remember it, and pull its listings from the onion in
    /// its record. A timeout falls back to one listing query on an introduction.
    pub(crate) fn open_store_listings(&self, id: [u8; 32]) -> Result<(), String> {
        {
            let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.note_opened(id)?;
        }
        if self.directory_sale_url(&StoreId::from_bytes(id)).is_none() {
            self.refresh_market();
            return Ok(());
        }
        let onion = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.stores.get(&id).and_then(|store| store.details.as_ref()).map(|details| details.onion.clone())
        };
        let state_dir = self.data_dir.join("tor-client");
        let primary = match onion.as_deref() {
            Some(onion) if !onion.is_empty() => match ask(&state_dir, onion, tormail::QUERY_LISTINGS, id) {
                Ok(Some(blob)) => split_listings(&blob),
                Ok(None) => Ok(Vec::new()),
                Err(err) => Err(err),
            },
            _ => Err("the store record has no onion".into()),
        };
        let fallback = self.introduction_listings(id);
        let lists = listings_after_dial(primary, fallback);
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).keep_listings(&id, &lists)?;
        self.refresh_market();
        Ok(())
    }

    fn introduction_listings(&self, id: [u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        if !self.local.introductions {
            return Err("introductions are off".into());
        }
        let hosts = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).introduction_hosts();
        let state_dir = self.data_dir.join("tor-client");
        let mut last = String::from("no introduction answered");
        for onion in hosts.iter().take(PASS_ATTEMPTS) {
            match ask(&state_dir, onion, tormail::QUERY_LISTINGS, id) {
                Ok(Some(blob)) => return split_listings(&blob),
                Ok(None) => return Ok(Vec::new()),
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    fn pass_introductions(&self, origin: usize) -> Vec<Introduction> {
        let state_dir = self.data_dir.join("tor-client");
        let hosts = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).introduction_hosts();
        crate::seeds::pass_hosts(&hosts, origin)
            .into_iter()
            .map(|onion| Introduction { onion, state_dir: state_dir.clone() })
            .collect()
    }

    /// Queue the own store record and `listing` for [`Self::flush_outbox`].
    /// The record is whatever this node has signed, including its onion.
    /// Writing again refreshes those files. Flush leaves them and retries.
    pub(crate) fn queue_outbox(&self, listing: &Listing) -> Result<(), String> {
        let store = self.own_record().ok_or_else(|| "the own store has no signed record yet".to_string())?;
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        replica.queue_outbox(&store, listing)
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
            replica.queue_outbox(&store, &listing).map_err(OrderError::Failed)?;
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
    replica.pull(&refs, &DOCUMENT_KEY, [0u8; 32])?;
    replica
        .head()
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
    json_body("200 OK", &node.directory_value(), origin)
}

/// `GET /directory` lists replicated stores by name. `DELETE /follow` drops a key this person added.
pub(crate) fn directory_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    json_body("200 OK", &node.directory_value(), origin)
}

pub(crate) fn unfollow_response(node: &Node, client: &Client, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_error("400 Bad Request", "Remove store takes a store key.", origin),
    };
    let Some(object) = value.as_object() else {
        return json_error("400 Bad Request", "Remove store takes a store key.", origin);
    };
    if object.len() != 1 || !object.contains_key("storeId") {
        return json_error("400 Bad Request", "Remove store takes a store key.", origin);
    }
    let Some(id) = object.get("storeId").and_then(Value::as_str).and_then(unhex32) else {
        return json_error("400 Bad Request", "The store key is 64 hex characters.", origin);
    };
    if let Err(err) = node.unfollow_store(id) {
        eprintln!("ob-node: unfollow: {err}");
        return json_error("500 Internal Server Error", "The store key could not be removed.", origin);
    }
    json_body("200 OK", &node.directory_value(), origin)
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
    if names_destination(&value) {
        return json_error("400 Bad Request", "A listing is signed here. An address was not accepted.", origin);
    }
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
    use openbazaar_protocol::{
        fixture_catalog, secret_from_label, xonly, Admission, ContentLabel, DirectoryTerms, ListingKind, SeedDocument, ADMISSION_LEN,
        FEE_SATS,
    };

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
        let terms = DirectoryTerms::standard(key);
        let older = SeedDocument::sign(1, vec![onion('a')], terms, None, &secret).unwrap();
        let newer = SeedDocument::sign(5, vec![onion('b'), onion('a')], terms, None, &secret).unwrap();
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
        peer.follow(*store.id.as_bytes()).unwrap();
        peer.pull(&[&stopped, &high], &key, [0u8; 32]).unwrap();
        assert_eq!(peer.document.as_ref().unwrap().sequence, 5);
        assert_eq!(peer.stores.len(), 1);
        assert!(!peer.listings.is_empty());

        peer.pull(&[&low, &stopped], &key, [0u8; 32]).unwrap();
        assert_eq!(peer.document.as_ref().unwrap().sequence, 5, "an older document and a stopped host do not replace it");
        assert!(!peer.listings.is_empty(), "a stopped introduction does not clear the listings already held");

        drop(high);
        drop(stopped);
        drop(peer);
        // The document file is signed by the test key, not DOCUMENT_KEY, so a
        // reload ignores it. Following the store is what keeps it in the index.
        let mut kept = Replica::open(&dir.0).unwrap();
        kept.follow(*store.id.as_bytes()).unwrap();
        let reloaded = Replica::open(&dir.0).unwrap();
        let mut fresh = Catalog { stores: vec![], moderator: catalog.moderator.clone(), listings: vec![] };
        reloaded.apply(&mut fresh, StoreId::from_bytes([9u8; 32]), |_| false, |_| false);
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
        {
            let replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            let outbox = replica.dir.join("outbox");
            let queued = fs::read(outbox.join("store")).unwrap();
            Store::decode(&queued).unwrap().verify().unwrap();
            let listings = fs::read_dir(&outbox).unwrap().flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("listing-")).count();
            assert_eq!(listings, 1, "a publish queues the signed listing for both introductions");
        }

        let extra = format!(r#"{{"storeId":"{}","onion":"not-a-destination"}}"#, "ab".repeat(32));
        let (status, _) = harness::request(&node, "POST", "/follow", &extra);
        assert!(status.contains("400"), "{status}");
        let (status, value) = harness::request(&node, "POST", "/follow", &format!(r#"{{"storeId":"{}"}}"#, "cd".repeat(32)));
        assert!(status.contains("200"), "{status} {value}");
    }

    #[test]
    fn a_shop_publish_queues_the_store_and_one_listing_and_refuses_an_address() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let body = r#"{"title":"Bacon","category":"Sporting Goods","kind":"physical","shipFrom":"US","description":"Smoked.","currency":"SAT","price":435,"shipping":0,"stock":1}"#;
        let (status, value) = harness::request(&node, "POST", "/store/listings", body);
        assert!(status.contains("200"), "{status} {value}");
        let record = node.own_record().unwrap().encode().unwrap();
        {
            let replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            let outbox = replica.dir.join("outbox");
            let queued = fs::read(outbox.join("store")).unwrap();
            assert_eq!(queued, record, "the outbox stores the current signed record");
            Store::decode(&queued).unwrap().verify().unwrap();
            let listings: Vec<_> = fs::read_dir(&outbox)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("listing-"))
                .collect();
            assert_eq!(listings.len(), 1, "a shop publish queues one signed listing");
            let listing = Listing::decode(&fs::read(listings[0].path()).unwrap()).unwrap();
            listing.verify().unwrap();
            assert_eq!((listing.title.as_str(), listing.store_id, listing.price_sats), ("Bacon", node.own_store, 435));
        }

        let named = r#"{"title":"Bacon","category":"Sporting Goods","kind":"physical","shipFrom":"US","description":"Smoked.","currency":"SAT","price":435,"shipping":0,"stock":1,"onion":"abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst.onion","url":"http://shop.example"}"#;
        let (status, _) = harness::request(&node, "POST", "/store/listings", named);
        assert!(status.contains("400"), "{status}");
        {
            let replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            let listings = fs::read_dir(replica.dir.join("outbox"))
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("listing-"))
                .count();
            assert_eq!(listings, 1, "a body naming an onion or url does not queue another listing");
        }

        let id = value["listingId"].as_str().unwrap();
        let edited = body.replace("\"price\":435", "\"price\":500");
        let (status, updated) = harness::request(&node, "PUT", &format!("/store/listings/{id}"), &edited);
        assert!(status.contains("200"), "{status} {updated}");
        let record = node.own_record().unwrap().encode().unwrap();
        let replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let outbox = replica.dir.join("outbox");
        assert_eq!(fs::read(outbox.join("store")).unwrap(), record, "saving again refreshes the store file");
        let listings: Vec<_> = fs::read_dir(&outbox)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("listing-"))
            .map(|entry| Listing::decode(&fs::read(entry.path()).unwrap()).unwrap())
            .collect();
        assert!(listings.iter().any(|listing| listing.price_sats == 500), "saving again queues the new signature");
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
    fn a_new_store_stays_hidden_until_the_next_sequence_and_a_bad_byte_is_dropped() {
        let secret = secret_from_label("inbox-store");
        let key = xonly(&secret);
        let onion_host = format!("{}.onion", "c".repeat(56));
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop",
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: onion_host.clone(),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile {
                    tagline: String::new(),
                    about: String::new(),
                    avatar: None,
                    banner: None,
                    shipping_policy: String::new(),
                    returns_policy: String::new(),
                    processing_time: String::new(),
                    ships_to: vec![],
                    created_on: 20261109,
                },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let listing = Listing::sign(
            store.id,
            "Cedar spoon",
            "Home",
            2_400,
            "Maine",
            "A small cedar spoon.",
            "Cedar Workshop",
            3,
            20261109,
            false,
            ListingKind::Digital,
            "",
            0,
            0,
            ContentLabel::GENERAL,
            &secret,
        )
        .unwrap();
        let terms = DirectoryTerms::standard(key);
        let empty = SeedDocument::sign(1, vec![onion('a')], terms, None, &secret).unwrap();
        let named = SeedDocument::sign(2, vec![onion('a'), onion('b')], terms, None, &secret).unwrap();
        let intro_dir = TempDir::new();
        let mut intro = Replica::open(&intro_dir.0).unwrap();
        intro.accept_document(&empty.encode().unwrap(), &key).unwrap();
        intro.accept_store(&store.encode().unwrap()).unwrap();
        intro.accept_listing(&listing.encode().unwrap()).unwrap();
        assert!(intro.answer(tormail::QUERY_STORE, store.id.as_bytes()).is_some(), "an introduction serves the bytes it holds");
        assert!(intro.accept_store(b"http://shop.example").is_err());
        assert!(intro.accept_listing(b"https://shop.example/item").is_err());
        let mut tampered = listing.encode().unwrap();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(intro.accept_listing(&tampered).is_err());

        let peer_dir = TempDir::new();
        let mut peer = Replica::open(&peer_dir.0).unwrap();
        peer.pull(&[&intro], &key, [0u8; 32]).unwrap();
        assert!(peer.stores.is_empty(), "a peer does not copy a store that has not paid the fee");
        assert_eq!(peer.document.as_ref().unwrap().sequence, 1);

        let other_dir = TempDir::new();
        let mut other = Replica::open(&other_dir.0).unwrap();
        other.accept_document(&empty.encode().unwrap(), &key).unwrap();
        other.accept_store(&store.encode().unwrap()).unwrap();
        other.accept_listing(&listing.encode().unwrap()).unwrap();
        let stopped = Held {
            document: empty.encode().unwrap(),
            store: Some(store.encode().unwrap()),
            listings: vec![listing.encode().unwrap()],
            up: false,
        };
        let admission = Admission::sign(*store.id.as_bytes(), [7u8; 32], 0, 50, &secret).unwrap();
        intro.accept_admission(&admission.encode().unwrap()).unwrap();
        other.accept_admission(&admission.encode().unwrap()).unwrap();
        let mut flipped = admission.encode().unwrap();
        let last = flipped.len() - 1;
        flipped[last] ^= 0xff;
        assert!(intro.accept_admission(&flipped).is_err(), "a flipped admission is not stored");
        assert!(intro.admissions.contains_key(store.id.as_bytes()));
        let _ = named;
        peer.pull(&[&stopped, &other], &key, [0u8; 32]).unwrap();
        assert_eq!(peer.document.as_ref().unwrap().sequence, 1, "admitting a store does not change the head sequence");
        assert!(peer.stores.contains_key(store.id.as_bytes()), "a second node shows a store no sequence names");
        assert!(peer.admissions.contains_key(store.id.as_bytes()));
        for n in 0..257u16 {
            let extra = secret_from_label(&format!("phase1-store-{n}"));
            let id = xonly(&extra);
            let record = Admission::sign(id, [n as u8; 32], 0, 50, &extra).unwrap();
            intro.accept_admission(&record.encode().unwrap()).unwrap();
        }
        assert!(intro.admissions.len() >= 257, "a 257th store is admitted");
        let page = intro.answer(tormail::QUERY_ADMISSIONS, &[0u8; 32]).unwrap();
        assert!(page.len() % ADMISSION_LEN == 0);
        assert!(page.len() / ADMISSION_LEN >= 257);
        assert!(!page.windows(ADMISSION_LEN).any(|chunk| chunk == flipped.as_slice()));
    }

    #[test]
    fn a_second_node_orders_a_named_store_without_a_url() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let secret = secret_from_label("named-sale");
        let onion_host = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion".to_string();
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop",
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: onion_host.clone(),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile {
                    tagline: String::new(),
                    about: String::new(),
                    avatar: None,
                    banner: None,
                    shipping_policy: String::new(),
                    returns_policy: String::new(),
                    processing_time: String::new(),
                    ships_to: vec![],
                    created_on: 20261109,
                },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let listing = Listing::sign(
            store.id,
            "Cedar spoon",
            "Home",
            2_400,
            "Maine",
            "A small cedar spoon.",
            "Cedar Workshop",
            3,
            20261109,
            false,
            ListingKind::Digital,
            "",
            0,
            0,
            ContentLabel::GENERAL,
            &secret,
        )
        .unwrap();
        let document = SeedDocument::sign(1, vec![onion('a')], DirectoryTerms::standard(xonly(&secret)), None, &secret).unwrap();
        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let buyer = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        {
            let mut replica = buyer.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_document(&document.encode().unwrap(), &xonly(&secret)).unwrap();
            replica.accept_store(&store.encode().unwrap()).unwrap();
            replica.accept_listing(&listing.encode().unwrap()).unwrap();
            replica.follow(*store.id.as_bytes()).unwrap();
            let own = buyer.own_store;
            replica.apply(&mut buyer.catalog.write().unwrap_or_else(|err| err.into_inner()), own, |item| item.id == own || crate::demo_secret(item).is_some(), |_| true);
        }
        let expected = format!("http://{onion_host}");
        assert_eq!(buyer.directory_sale_url(&store.id).as_deref(), Some(expected.as_str()));
        let hash = listing.content_hash().unwrap().to_hex();
        let body = format!(
            r#"{{"deliveryNote":"Send the file.","packages":[{{"settlement":"direct","lines":[{{"listingHash":"{hash}","quantity":1}}]}}]}}"#
        );
        let (status, value) = harness::request(&buyer, "POST", "/checkout", &body);
        let text = value.to_string();
        assert!(!text.to_ascii_lowercase().contains("store node address"), "{status} {text}");
        assert!(
            status.contains("200") || text.contains("no address for the seller's reply"),
            "the order uses the record's onion, not a typed address: {status} {text}"
        );
        let refused = format!(r#"{{"title":"Spoon","category":"Home","shipFrom":"Maine","description":"Wood.","priceSats":1,"stock":1,"onion":"{onion_host}"}}"#);
        let (status, _) = harness::request(&buyer, "POST", "/listings", &refused);
        assert!(status.contains("400"), "{status}");
    }

    #[test]
    fn the_shipped_introductions_are_v3_onions() {
        for onion in crate::seeds::INTRODUCTIONS {
            tormail::OnionAddress::parse(onion).unwrap();
        }
    }

    fn terms_for(secret: &bitcoin::secp256k1::SecretKey) -> DirectoryTerms {
        DirectoryTerms::standard(xonly(secret))
    }

    #[test]
    fn phase2_a_running_introduction_learns_a_third_onion_from_the_head() {
        let secret = secret_from_label("phase2-head");
        let key = xonly(&secret);
        let third = onion('c');
        let first = SeedDocument::sign(1, vec![onion('a'), onion('b')], terms_for(&secret), None, &secret).unwrap();
        let next = SeedDocument::sign(2, vec![onion('a'), onion('b'), third.clone()], terms_for(&secret), None, &secret).unwrap();
        let dir = TempDir::new();
        let mut intro = Replica::open(&dir.0).unwrap();
        assert_eq!(
            intro.introduction_hosts(),
            crate::seeds::INTRODUCTIONS.iter().map(|onion| (*onion).to_string()).collect::<Vec<_>>()
        );
        intro.accept_document(&first.encode().unwrap(), &key).unwrap();
        assert_eq!(intro.introduction_hosts().len(), 2);
        intro.accept_document(&next.encode().unwrap(), &key).unwrap();
        assert!(intro.introduction_hosts().contains(&third), "a running introduction keeps the new head without a restart");
        assert!(!crate::seeds::INTRODUCTIONS.iter().any(|onion| *onion == third));
        let peer_dir = TempDir::new();
        let mut peer = Replica::open(&peer_dir.0).unwrap();
        peer.accept_document(&next.encode().unwrap(), &key).unwrap();
        assert_eq!(peer.introduction_hosts(), next.introductions);
        assert!(peer.introduction_hosts().iter().all(|host| !crate::seeds::INTRODUCTIONS.contains(&host.as_str())));
        let down = Held { document: next.encode().unwrap(), store: None, listings: vec![], up: false };
        let up = Held { document: next.encode().unwrap(), store: None, listings: vec![], up: true };
        let fresh_dir = TempDir::new();
        let mut fresh = Replica::open(&fresh_dir.0).unwrap();
        fresh.pull(&[&down, &up, &up], &key, [0u8; 32]).unwrap();
        assert_eq!(fresh.document.as_ref().unwrap().sequence, 2, "one of three introductions down still returns the head");
        assert!(fresh.introduction_hosts().contains(&third));
    }

    #[test]
    fn phase3_listings_follow_opens_and_search_stays_on_held_records() {
        let secret = secret_from_label("phase3-seller");
        let key = xonly(&secret);
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop",
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: onion('d'),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile {
                    tagline: "Wooden tools".into(),
                    about: String::new(),
                    avatar: None,
                    banner: None,
                    shipping_policy: String::new(),
                    returns_policy: String::new(),
                    processing_time: String::new(),
                    ships_to: vec![],
                    created_on: 20261109,
                },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let listing = Listing::sign(
            store.id, "Cedar spoon", "Home", 2_400, "Maine", "A small cedar spoon.", "Cedar Workshop", 3, 20261109, false,
            ListingKind::Digital, "", 0, 0, ContentLabel::GENERAL, &secret,
        )
        .unwrap();
        let followed_secret = secret_from_label("phase3-followed");
        let followed = openbazaar_protocol::Store::sign(
            "Maple Workshop",
            "Vermont",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            store.details.clone().unwrap(),
            &followed_secret,
        )
        .unwrap();
        let followed_listing = Listing::sign(
            followed.id, "Maple spoon", "Home", 2_600, "Vermont", "A small maple spoon.", "Maple Workshop", 2, 20261109, false,
            ListingKind::Digital, "", 0, 0, ContentLabel::GENERAL, &followed_secret,
        )
        .unwrap();
        let head = SeedDocument::sign(1, vec![onion('a')], terms_for(&secret), None, &secret).unwrap();
        let intro_dir = TempDir::new();
        let mut intro = Replica::open(&intro_dir.0).unwrap();
        intro.accept_document(&head.encode().unwrap(), &key).unwrap();
        for (shop, item, signer) in [(&store, &listing, &secret), (&followed, &followed_listing, &followed_secret)] {
            intro.accept_store(&shop.encode().unwrap()).unwrap();
            intro.accept_listing(&item.encode().unwrap()).unwrap();
            let admission = Admission::sign(*shop.id.as_bytes(), [9u8; 32], 0, 10, signer).unwrap();
            intro.accept_admission(&admission.encode().unwrap()).unwrap();
        }
        let peer_dir = TempDir::new();
        let mut peer = Replica::open(&peer_dir.0).unwrap();
        let own_secret = secret_from_label("phase3-own");
        let own = xonly(&own_secret);
        let own_store = openbazaar_protocol::Store::sign(
            "Own Workshop", "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            store.details.clone().unwrap(),
            &own_secret,
        )
        .unwrap();
        let own_listing = Listing::sign(
            own_store.id, "Own spoon", "Home", 100, "Maine", "Made here.", "Own Workshop", 1, 20261109, false,
            ListingKind::Digital, "", 0, 0, ContentLabel::GENERAL, &own_secret,
        )
        .unwrap();
        peer.save_store(&own_store).unwrap();
        peer.save_listing(*own_listing.content_hash().unwrap().as_bytes(), &own_listing).unwrap();
        peer.follow(*followed.id.as_bytes()).unwrap();
        peer.pull(&[&intro], &key, own).unwrap();
        let ids_of = |replica: &Replica| {
            replica.listings.values().map(|listing| *listing.store_id.as_bytes()).collect::<std::collections::BTreeSet<_>>()
        };
        let held = ids_of(&peer);
        assert!(held.contains(&own), "the own store's listing is on disk");
        assert!(held.contains(followed.id.as_bytes()), "a follow's listings are on disk");
        assert!(!held.contains(store.id.as_bytes()), "an admitted store that was not opened has no listing file");
        let live = |_: &Admission| true;
        let (names, titles) = peer.search("Cedar", &own, &live);
        assert_eq!(names, vec!["Cedar Workshop".to_string()]);
        assert!(titles.is_empty(), "a listing title does not match before the store is opened");
        let primary = Ok(vec![listing.encode().unwrap()]);
        assert_eq!(listings_after_dial(primary, Err("the introduction is not asked".into())).len(), 1);
        let timed_out: Result<Vec<Vec<u8>>, String> = Err("introduction timed out".into());
        let fallback = intro.listing_bytes(store.id.as_bytes());
        let lists = listings_after_dial(timed_out, fallback);
        peer.note_opened(*store.id.as_bytes()).unwrap();
        peer.keep_listings(store.id.as_bytes(), &lists).unwrap();
        assert!(ids_of(&peer).contains(store.id.as_bytes()));
        let (names, titles) = peer.search("Cedar spoon", &own, &live);
        assert!(names.is_empty());
        assert_eq!(titles, vec!["Cedar spoon".to_string()]);
        let mut catalog = Catalog { stores: vec![], moderator: fixture_catalog().moderator, listings: vec![] };
        peer.apply(&mut catalog, StoreId::from_bytes(own), |_| false, |_| true);
        assert!(catalog.stores.iter().any(|item| item.name == "Cedar Workshop"));
        assert!(catalog.listings.iter().any(|item| item.title == "Cedar spoon"));
        assert!(catalog.listings.iter().any(|item| item.title == "Maple spoon"));
    }

    /// Admission is served on every pass. The store record stays hidden until
    /// the test releases it, and every store or listing ask is recorded.
    struct HeldBack {
        document: Vec<u8>,
        admission: Vec<u8>,
        store: std::cell::RefCell<Option<Vec<u8>>>,
        asked: std::cell::RefCell<Vec<[u8; 32]>>,
        listings: std::cell::RefCell<Vec<[u8; 32]>>,
    }

    impl Source for HeldBack {
        fn seed_bytes(&self) -> Result<Vec<u8>, String> {
            Ok(self.document.clone())
        }

        fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
            self.asked.borrow_mut().push(*id);
            Ok(self.store.borrow().clone())
        }

        fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
            self.listings.borrow_mut().push(*id);
            Ok(Vec::new())
        }

        fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
            match Admission::peek_store_id(&self.admission) {
                Some(id) if id > *cursor => Ok(Some(self.admission.clone())),
                _ => Ok(Some(Vec::new())),
            }
        }
    }

    #[test]
    fn a_later_pass_asks_for_an_admitted_store_that_still_has_no_record() {
        let secret = secret_from_label("later-pass-store");
        let key = xonly(&secret);
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop",
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: onion('f'),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile {
                    tagline: String::new(),
                    about: String::new(),
                    avatar: None,
                    banner: None,
                    shipping_policy: String::new(),
                    returns_policy: String::new(),
                    processing_time: String::new(),
                    ships_to: vec![],
                    created_on: 20261109,
                },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let id = *store.id.as_bytes();
        let admission = Admission::sign(id, [9u8; 32], 0, 10, &secret).unwrap();
        let head = SeedDocument::sign(1, vec![onion('a')], terms_for(&secret), None, &secret).unwrap();
        let source = HeldBack {
            document: head.encode().unwrap(),
            admission: admission.encode().unwrap(),
            store: std::cell::RefCell::new(None),
            asked: std::cell::RefCell::new(Vec::new()),
            listings: std::cell::RefCell::new(Vec::new()),
        };
        let dir = TempDir::new();
        let mut peer = Replica::open(&dir.0).unwrap();
        peer.pull(&[&source], &key, [0u8; 32]).unwrap();
        assert!(peer.admissions.contains_key(&id), "the first pass admits the store");
        assert!(!peer.stores.contains_key(&id), "the first pass does not keep a store record that was not delivered");
        source.asked.borrow_mut().clear();
        let record = store.encode().unwrap();
        *source.store.borrow_mut() = Some(record.clone());
        peer.pull(&[&source], &key, [0u8; 32]).unwrap();
        assert!(
            source.asked.borrow().contains(&id),
            "a later pass asks for an admitted store that still has no record"
        );
        assert_eq!(peer.stores.get(&id).unwrap().name, "Cedar Workshop");
        assert_eq!(fs::read(peer.store_path(&id)).unwrap(), record, "the later pass keeps the store record");
        let mut flipped = record.clone();
        flipped[20] ^= 0xff;
        assert!(peer.accept_store(&flipped).is_err(), "a flipped store record is refused");
        assert_eq!(fs::read(peer.store_path(&id)).unwrap(), record, "a flipped record leaves the last good record in place");
        *source.store.borrow_mut() = Some(flipped);
        source.asked.borrow_mut().clear();
        peer.pull(&[&source], &key, [0u8; 32]).unwrap();
        assert_eq!(peer.stores.get(&id).unwrap().name, "Cedar Workshop");
        assert_eq!(fs::read(peer.store_path(&id)).unwrap(), record, "a later pass does not replace the kept record with a flipped one");
        assert!(
            source.listings.borrow().is_empty(),
            "listings stay limited to the own store, follows, and the last 50 opened"
        );
    }

    #[test]
    fn phase1_and_phase4_the_fee_admits_one_store_and_a_bad_byte_stays_out() {
        use crate::chain::ChainSource;
        use crate::harness;
        use crate::network::NetworkMode;
        use bitcoin::absolute::LockTime;
        use bitcoin::hashes::Hash;
        use bitcoin::transaction::Version;
        use bitcoin::{Amount, OutPoint, Sequence, Transaction, TxIn, TxOut, Witness};

        let secret = secret_from_label("phase4-fee");
        let key = xonly(&secret);
        let chain = harness::MockChain::new();
        let store_id = key;
        let head = SeedDocument::sign(3, vec![onion('a')], DirectoryTerms::standard(key), None, &secret).unwrap();
        let pay = |sats: u64, id: &[u8; 32]| {
            let tx = Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::default(),
                    script_sig: bitcoin::ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::default(),
                }],
                output: vec![
                    TxOut { value: Amount::from_sat(sats), script_pubkey: crate::admission::fee_script(&key, NetworkMode::Testnet4.bitcoin()).unwrap() },
                    TxOut { value: Amount::ZERO, script_pubkey: crate::admission::fee_return(id).unwrap() },
                ],
            };
            chain.broadcast(&tx).unwrap();
            chain.mine();
            tx
        };
        assert_eq!(page_cap(), tormail::MAX_QUERY_LEN / ADMISSION_LEN);
        assert_eq!(20_000 * ADMISSION_LEN, 2_820_000);
        assert_eq!(20_000_usize.div_ceil(page_cap()), 3);
        let tx = pay(FEE_SATS, &store_id);
        assert!(tx.output.iter().any(|output| output.value.to_sat() >= FEE_SATS));
        assert_eq!(tx.output.len(), 2, "the fee transaction has a payment and an OP_RETURN, not an order");
        let height = chain.tip();
        let admission = Admission::sign(store_id, tx.compute_txid().to_byte_array(), 0, height, &secret).unwrap();
        assert!(crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &admission));
        let short = pay(FEE_SATS - 1, &store_id);
        let short_admission = Admission::sign(store_id, short.compute_txid().to_byte_array(), 0, chain.tip(), &secret).unwrap();
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &short_admission), "a short fee is not listed");
        let missing = Admission::sign(store_id, [8u8; 32], 0, height, &secret).unwrap();
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &missing), "a missing transaction is not listed");
        let other = secret_from_label("phase4-other");
        let other_id = xonly(&other);
        let reused = Admission::sign(other_id, tx.compute_txid().to_byte_array(), 0, height, &other).unwrap();
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &reused), "one fee admits one store id");
        assert!(crate::reputation::direct_pay_allowed(0).is_err(), "the fee does not unlock direct pay");

        let dir = TempDir::new();
        let mut intro = Replica::open(&dir.0).unwrap();
        intro.accept_document(&head.encode().unwrap(), &key).unwrap();
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop", "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32], onion: onion('e'), moderators: vec![],
                profile: openbazaar_protocol::StoreProfile { tagline: String::new(), about: String::new(), avatar: None, banner: None, shipping_policy: String::new(), returns_policy: String::new(), processing_time: String::new(), ships_to: vec![], created_on: 20261109 },
                accepts_xmr: false, accepts_lightning: false, accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        intro.accept_store(&store.encode().unwrap()).unwrap();
        let kept = Listing::sign(
            store.id, "Cedar spoon", "Home", 2_400, "Maine", "A small cedar spoon.", "Cedar Workshop", 3, 20261109, false,
            ListingKind::Digital, "", 0, 0, ContentLabel::GENERAL, &secret,
        )
        .unwrap();
        intro.accept_listing(&kept.encode().unwrap()).unwrap();
        let mut flipped_listing = kept.encode().unwrap();
        let listing_last = flipped_listing.len() - 1;
        flipped_listing[listing_last] ^= 0xff;
        assert!(intro.accept_listing(&flipped_listing).is_err());
        assert_eq!(intro.listings.len(), 1, "a failed listing verify leaves the last good listing");
        intro.accept_admission(&admission.encode().unwrap()).unwrap();
        let mut flipped_store = store.encode().unwrap();
        flipped_store[20] ^= 0xff;
        assert!(intro.accept_store(&flipped_store).is_err());
        assert_eq!(intro.stores.get(store.id.as_bytes()).unwrap().name, "Cedar Workshop");
        let mut catalog = Catalog { stores: vec![], moderator: fixture_catalog().moderator, listings: vec![] };
        let live = |record: &Admission| crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, record);
        intro.apply(&mut catalog, StoreId::from_bytes([1u8; 32]), |_| false, &live);
        assert!(catalog.stores.iter().any(|item| item.name == "Cedar Workshop"));
        assert!(catalog.listings.is_empty(), "held back listings are not goods and checkout is not offered");
        let hidden = TempDir::new();
        let mut bare = Replica::open(&hidden.0).unwrap();
        bare.accept_document(&head.encode().unwrap(), &key).unwrap();
        bare.accept_admission(&admission.encode().unwrap()).unwrap();
        let mut empty = Catalog { stores: vec![], moderator: catalog.moderator.clone(), listings: vec![] };
        bare.apply(&mut empty, StoreId::from_bytes([1u8; 32]), |_| false, &live);
        assert!(empty.stores.is_empty(), "an admission with no store record is not a shop");
        *chain.0.tip.lock().unwrap() = height.saturating_add(head.terms.epoch_blocks).saturating_add(1);
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &admission), "an old admission leaves the index");
        bare.accept_store(&store.encode().unwrap()).unwrap();
        bare.follow(store_id).unwrap();
        assert!(bare.show_store(&store_id, &|_| false), "a followed id stays wanted after the epoch");
    }

    #[test]
    fn phase1_the_seller_pays_the_fee_from_its_wallet_without_a_new_sequence() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        harness::fund_node_wallet(&node, &chain, 200_000);
        node.sync_buyer_wallet().unwrap();
        let secret = secret_from_label("seller-fee-head");
        let head = SeedDocument::sign(4, vec![onion('a')], DirectoryTerms::standard(xonly(&secret)), None, &secret).unwrap();
        let sequence = head.sequence;
        {
            let mut replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_document(&head.encode().unwrap(), &xonly(&secret)).unwrap();
        }
        node.publish_admission().unwrap();
        assert!(chain.broadcasts().is_empty(), "a node that has not chosen to sell pays no fee");
        node.set_role(crate::store_profile::Role::Seller).unwrap();
        node.publish_admission().unwrap();
        assert!(node.seed_replica.lock().unwrap().read_outbox("admission").is_none(), "the admission waits for fee_confs");
        assert_eq!(chain.broadcasts().len(), 1);
        let tx = chain.broadcasts()[0].clone();
        let label = crate::admission::fee_return(node.own_store.as_bytes()).unwrap();
        assert!(tx.output.iter().any(|output| output.script_pubkey == label));
        assert!(tx.output.iter().any(|output| output.value.to_sat() >= FEE_SATS));
        chain.mine();
        node.publish_admission().unwrap();
        let bytes = node.seed_replica.lock().unwrap().read_outbox("admission").expect("the admission is queued");
        let admission = Admission::decode(&bytes).unwrap();
        assert_eq!(admission.store_id, *node.own_store.as_bytes());
        assert!(crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &admission));
        assert!(node.seed_replica.lock().unwrap().read_outbox("store").is_some());
        assert_eq!(node.seed_replica.lock().unwrap().head().unwrap().sequence, sequence);
        assert!(node.catalog().stores.iter().any(|store| store.id == node.own_store), "the own node still shows its store");
        assert!(node.directory_sale_url(&node.own_store).is_none());
        let demo = node.catalog().stores.iter().find(|store| crate::demo_secret(store).is_some()).unwrap().id;
        assert!(node.directory_sale_url(&demo).is_none());
    }

    #[test]
    fn a_node_with_introductions_off_does_not_ask_one_for_listings() {
        use crate::harness::TempDir;
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let mut local = crate::LocalConfig::headless();
        local.introductions = false;
        let node = Node::open_local(&dir.0, crate::load_catalog(NetworkMode::Testnet4).unwrap(), crate::NodeConfig::offline(NetworkMode::Testnet4), local).unwrap();
        assert!(node.introduction_listings([7; 32]).is_err());
        assert!(!node.data_dir.join("tor-client").exists(), "no Tor client was opened to dial one");
    }
}
