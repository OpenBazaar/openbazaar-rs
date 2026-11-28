//! Directory replication.
//!
//! A peer asks up to three introductions for the head and keeps the highest
//! sequence that verifies under the shipped key and extends the held fee
//! terms ([`crate::directory_head`]). Stores join with an
//! admission, not with a new sequence. Store records are wanted for an
//! admitted or followed id, and for a store the directory's signed admission
//! snapshot names. Listing bodies are wanted for the own store, followed ids,
//! and the 50 stores last opened here. A store's signed catalog index names
//! the listing hashes for its current record; a listing the index leaves out
//! is dropped. A source that is down is skipped. A bad byte does not delete
//! the last good record.
//!
//! Nothing here accepts a client-supplied onion. Add-store takes a store key.
//! The record is kept only when its signature matches that key.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use bitcoin::secp256k1::{Secp256k1, SecretKey};
use openbazaar_protocol::{
    Admission, AdmissionIndex, Catalog, CatalogIndex, Listing, SeedDocument, Store, StoreId, ADMISSION_LEN,
};
use serde_json::{json, Value};

use crate::directory_head::Heads;
use crate::http::{json_body, json_error, local_guard, Client};
use crate::orders::OrderError;
use crate::seeds::DOCUMENT_KEY;
use crate::{listing_value, Error, Node};

const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(60);
const FRAME_VERSION: u8 = 1;
const QUERY_MAGIC: &[u8; 4] = b"OBQR";
const SUBMIT_MAGIC: &[u8; 4] = b"OBSB";
static NEXT_DIR_STREAM: AtomicU64 = AtomicU64::new(0);

/// One compiled introduction. A checksum that Arti refuses is "down": the
/// pull skips it and keeps what is already stored.
struct Introduction {
    onion: String,
    state_dir: PathBuf,
    proxy: Option<String>,
}

impl Source for Introduction {
    fn seed_bytes(&self) -> Result<Vec<u8>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_SEED, [0u8; 32])?
            .ok_or_else(|| "introduction has no document".into())
    }

    fn head_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_HEAD, [0u8; 32])
    }

    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_STORE, *id)
    }

    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        match ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_LISTINGS, *id)? {
            Some(blob) => split_listings(&blob),
            None => Ok(Vec::new()),
        }
    }

    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_ADMISSIONS, *cursor)
    }

    fn catalog_index_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_CATALOG_INDEX, *id)
    }

    fn admission_index_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        ask(&self.state_dir, &self.onion, self.proxy.as_deref(), tormail::QUERY_ADMISSION_INDEX, [0u8; 32])
    }
}

fn socks5_connect(proxy: &str, host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let addr = crate::egress::proxy_addr(proxy)?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|err| err.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|err| err.to_string())?;

    // Offer no-auth (0) and username/password (2) so Tor's IsolateSOCKSAuth gives a fresh circuit.
    stream.write_all(&[5, 2, 0, 2]).map_err(|err| err.to_string())?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method).map_err(|err| err.to_string())?;
    if method[0] != 5 {
        return Err("not a SOCKS5 proxy".into());
    }
    if method[1] == 2 {
        let id = NEXT_DIR_STREAM.fetch_add(1, Ordering::Relaxed);
        let user = format!("ob-dir-{}-{id}", std::process::id());
        let pass = b"x";
        let mut auth = Vec::with_capacity(3 + user.len() + pass.len());
        auth.push(1);
        auth.push(u8::try_from(user.len()).map_err(|_| "SOCKS5 username too long".to_string())?);
        auth.extend_from_slice(user.as_bytes());
        auth.push(pass.len() as u8);
        auth.extend_from_slice(pass);
        stream.write_all(&auth).map_err(|err| err.to_string())?;
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).map_err(|err| err.to_string())?;
        if reply[1] != 0 {
            return Err("SOCKS5 auth rejected".into());
        }
    } else if method[1] != 0 {
        return Err(format!("SOCKS5 proxy chose unsupported auth method {}", method[1]));
    }

    let host_bytes = host.as_bytes();
    let host_len = u8::try_from(host_bytes.len()).map_err(|_| "onion host too long".to_string())?;
    let mut cmd = Vec::with_capacity(7 + host_bytes.len());
    cmd.extend_from_slice(&[5, 1, 0, 3, host_len]);
    cmd.extend_from_slice(host_bytes);
    cmd.push((port >> 8) as u8);
    cmd.push(port as u8);
    stream.write_all(&cmd).map_err(|err| err.to_string())?;

    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).map_err(|err| err.to_string())?;
    if hdr[0] != 5 || hdr[1] != 0 {
        return Err(format!("SOCKS5 CONNECT failed with code {}", hdr[1]));
    }
    match hdr[3] {
        1 => {
            let mut skip = [0u8; 6];
            stream.read_exact(&mut skip).map_err(|err| err.to_string())?;
        }
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).map_err(|err| err.to_string())?;
            let mut skip = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut skip).map_err(|err| err.to_string())?;
        }
        4 => {
            let mut skip = [0u8; 18];
            stream.read_exact(&mut skip).map_err(|err| err.to_string())?;
        }
        other => return Err(format!("SOCKS5 unknown address type {other}")),
    }
    Ok(stream)
}

fn submit_via_socks5(proxy: &str, onion: &tormail::OnionAddress, kind: u8, body: &[u8]) -> Result<bool, String> {
    if body.len() > tormail::MAX_QUERY_LEN {
        return Err("directory submission too large".into());
    }
    let len = u32::try_from(body.len()).map_err(|_| "directory submission too large".to_string())?;
    let mut stream = socks5_connect(proxy, onion.as_str(), tormail::ORDER_PORT, DIRECTORY_TIMEOUT)?;
    stream.write_all(SUBMIT_MAGIC).map_err(|err| err.to_string())?;
    stream.write_all(&[FRAME_VERSION, kind]).map_err(|err| err.to_string())?;
    stream.write_all(&len.to_le_bytes()).map_err(|err| err.to_string())?;
    stream.write_all(body).map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())?;
    let mut status = [0u8; 1];
    stream.read_exact(&mut status).map_err(|err| err.to_string())?;
    match status[0] {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err("malformed directory submission reply".into()),
    }
}

fn ask_via_socks5(proxy: &str, onion: &tormail::OnionAddress, kind: u8, id: [u8; 32]) -> Result<Option<Vec<u8>>, String> {
    let mut stream = socks5_connect(proxy, onion.as_str(), tormail::ORDER_PORT, DIRECTORY_TIMEOUT)?;
    stream.write_all(QUERY_MAGIC).map_err(|err| err.to_string())?;
    stream.write_all(&[FRAME_VERSION, kind]).map_err(|err| err.to_string())?;
    stream.write_all(&id).map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())?;
    let mut status = [0u8; 1];
    stream.read_exact(&mut status).map_err(|err| err.to_string())?;
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes).map_err(|err| err.to_string())?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > tormail::MAX_QUERY_LEN {
        return Err("directory reply too large".into());
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut body).map_err(|err| err.to_string())?;
    }
    match status[0] {
        0 => Ok(Some(body)),
        1 => Ok(None),
        _ => Err("malformed directory query reply".into()),
    }
}

fn submit_record(state_dir: &Path, onion: &str, proxy: Option<&str>, kind: u8, body: &[u8]) -> Result<bool, String> {
    let onion = tormail::OnionAddress::parse(onion).map_err(|err| err.to_string())?;
    if let Some(proxy) = proxy {
        return submit_via_socks5(proxy, &onion, kind, body);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(async {
            let client = tormail::open_client(state_dir)?;
            tokio::time::timeout(
                DIRECTORY_TIMEOUT,
                tormail::submit_directory(&client, &onion, kind, body),
            )
            .await
            .map_err(|_| tormail::Error::Transport("introduction timed out"))?
        })
        .map_err(|err| err.to_string())
}

fn ask(state_dir: &Path, onion: &str, proxy: Option<&str>, kind: u8, id: [u8; 32]) -> Result<Option<Vec<u8>>, String> {
    let onion = tormail::OnionAddress::parse(onion).map_err(|err| err.to_string())?;
    if let Some(proxy) = proxy {
        return ask_via_socks5(proxy, &onion, kind, id);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime
        .block_on(async {
            let client = tormail::open_client(state_dir)?;
            tokio::time::timeout(DIRECTORY_TIMEOUT, tormail::ask_directory(&client, &onion, kind, id))
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
    /// The version 3 head. `Ok(None)` means this source does not serve one,
    /// so the pass asks [`Source::seed_bytes`] instead.
    fn head_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String>;
    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String>;
    /// One page of admissions with store ids greater than `cursor`.
    /// `Ok(None)` means this source has none. An error means it did not answer.
    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        let _ = cursor;
        Ok(None)
    }

    /// The store's signed catalog index. `Ok(None)` means this source has none.
    fn catalog_index_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        let _ = id;
        Ok(None)
    }

    /// The directory's signed admission snapshot. `Ok(None)` means this source has none.
    fn admission_index_bytes(&self) -> Result<Option<Vec<u8>>, String> {
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
    heads: Heads,
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
    /// Highest signed catalog index kept for each store.
    catalog_indexes: BTreeMap<[u8; 32], CatalogIndex>,
    /// Highest signed admission snapshot. Used only when it names the held head.
    admission_index: Option<AdmissionIndex>,
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
        fs::create_dir_all(dir.join("indexes")).map_err(|err| Error::Detail(err.to_string()))?;
        let mut replica = Self {
            heads: Heads::load(&dir, &DOCUMENT_KEY),
            dir,
            followed: BTreeSet::new(),
            inbox: BTreeSet::new(),
            admissions: BTreeMap::new(),
            cursor: [0u8; 32],
            synced: false,
            opened: Vec::new(),
            stores: BTreeMap::new(),
            listings: BTreeMap::new(),
            live: BTreeSet::new(),
            catalog_indexes: BTreeMap::new(),
            admission_index: None,
        };
        replica.load().map_err(|err| Error::Detail(err))?;
        Ok(replica)
    }

    pub(crate) fn head(&self) -> Option<SeedDocument> {
        self.heads.document().cloned()
    }

    /// The last head this node refused, with the reason.
    pub(crate) fn head_refused(&self) -> Option<(u64, String)> {
        self.heads.refused().cloned()
    }

    /// Hosts to dial. A saved head supplies its own list. The compiled onions
    /// are only the bootstrap for an empty `document.obsd`.
    pub(crate) fn introduction_hosts(&self) -> Vec<String> {
        match self.heads.document() {
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
        self.pull_at(sources, key, own, None)
    }

    /// [`Replica::pull`] with the chain tip, which a head that drops fee terms needs.
    pub(crate) fn pull_at(&mut self, sources: &[&dyn Source], key: &[u8; 32], own: [u8; 32], tip: Option<u32>) -> Result<(), String> {
        pull_into(&RefCell::new(self), sources, key, own, tip)
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
                self.insert_admission(admission)?;
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

    /// Every admitted, followed, or snapshot-named id with no store file.
    /// A missing record stays wanted on the next pass.
    fn wanted_stores(&self) -> BTreeSet<[u8; 32]> {
        let mut ids: BTreeSet<[u8; 32]> = self.admissions.keys().chain(self.followed.iter()).copied().collect();
        if let Some(index) = self.current_snapshot() {
            ids.extend(index.stores.iter().copied());
        }
        ids.retain(|id| !self.stores.contains_key(id));
        ids
    }

    /// The signed admission snapshot for the head this node holds, or nothing.
    pub(crate) fn admission_record(&self) -> Option<Vec<u8>> {
        self.current_snapshot()?.encode().ok()
    }

    /// Store id and sale URL for each id the current snapshot names. The onion
    /// comes from the store record this node already verified. No listing.
    pub(crate) fn admission_addresses(&self) -> Vec<(String, String)> {
        let Some(index) = self.current_snapshot() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for id in &index.stores {
            let onion = self
                .stores
                .get(id)
                .and_then(|store| store.details.as_ref())
                .map(|details| details.onion.as_str())
                .unwrap_or("");
            if let Some(url) = sale_url(onion) {
                rows.push((hex(id), url));
            }
        }
        rows
    }

    /// The admission snapshot that accompanies the head this node holds.
    fn current_snapshot(&self) -> Option<&AdmissionIndex> {
        let head = self.heads.document()?;
        self.admission_index.as_ref().filter(|index| index.signer == head.signer && index.head_sequence == head.sequence)
    }

    fn snapshot_names(&self, id: &[u8; 32]) -> bool {
        self.current_snapshot().is_some_and(|index| index.stores.binary_search(id).is_ok())
    }

    pub(crate) fn admissions(&self) -> Vec<Admission> {
        self.admissions.values().cloned().collect()
    }

    pub(crate) fn admission_index_sequence(&self) -> u64 {
        self.admission_index.as_ref().map_or(0, |index| index.sequence)
    }

    pub(crate) fn admission_state(&self) -> Result<(u64, u64, Vec<[u8; 32]>), String> {
        let head = self
            .head()
            .ok_or_else(|| "no directory head in data dir; run fetch-seed first or pass --head-sequence".to_string())?;
        let next = self.admission_index_sequence().saturating_add(1);
        let mut stores: Vec<[u8; 32]> = self.admissions().into_iter().map(|a| a.store_id).collect();
        stores.sort();
        stores.dedup();
        Ok((head.sequence, next, stores))
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
        for id in self.admissions.keys() {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        ids
    }

    pub(crate) fn answer(&self, kind: u8, id: &[u8; 32]) -> Option<Vec<u8>> {
        match kind {
            tormail::QUERY_SEED => self.heads.seed_bytes(),
            tormail::QUERY_HEAD => self.heads.head_bytes(),
            tormail::QUERY_STORE => self.stores.get(id)?.encode().ok(),
            tormail::QUERY_CATALOG_INDEX => self.catalog_indexes.get(id).and_then(|index| index.encode().ok()),
            tormail::QUERY_ADMISSION_INDEX => self.admission_index.as_ref().and_then(|index| index.encode().ok()),
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
        if !self.index_allows(&id, hash.as_bytes()) {
            return Err("that listing is not in the store's catalog index".into());
        }
        self.save_listing(*hash.as_bytes(), &listing)
    }

    /// Keep a verified admission. A flipped byte or an older block height does
    /// not replace a newer valid admission for the same store.
    pub(crate) fn accept_admission(&mut self, bytes: &[u8]) -> Result<(), String> {
        let admission = Admission::decode(bytes).map_err(|err| err.to_string())?;
        self.insert_admission(admission)
    }

    fn insert_admission(&mut self, admission: Admission) -> Result<(), String> {
        let id = admission.store_id;
        if let Some(held) = self.admissions.get(&id) {
            if admission.height < held.height || *held == admission {
                return Ok(());
            }
        }
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
            if !self.index_allows(id, hash.as_bytes()) {
                continue;
            }
            self.save_listing(*hash.as_bytes(), &listing)?;
        }
        Ok(())
    }

    /// Keep a catalog index signed by the store it names. A lower sequence stays.
    /// A bad signature is not stored.
    pub(crate) fn accept_catalog_index(&mut self, bytes: &[u8]) -> Result<(), String> {
        let index = CatalogIndex::decode(bytes).map_err(|err| err.to_string())?;
        index.verify().map_err(|err| err.to_string())?;
        self.keep_catalog_index(index)
    }

    fn keep_catalog_index(&mut self, index: CatalogIndex) -> Result<(), String> {
        if self.catalog_indexes.get(&index.store_id).is_some_and(|held| index.sequence <= held.sequence) {
            return Ok(());
        }
        self.retract_absent(&index);
        let bytes = index.encode().map_err(|err| err.to_string())?;
        atomic(&self.index_path(&index.store_id), &bytes)?;
        self.catalog_indexes.insert(index.store_id, index);
        Ok(())
    }

    /// Keep an admission snapshot signed by `key`. A lower sequence stays.
    pub(crate) fn accept_admission_index(&mut self, bytes: &[u8], key: &[u8; 32]) -> Result<(), String> {
        let index = AdmissionIndex::decode(bytes).map_err(|err| err.to_string())?;
        index.verify_signer(key).map_err(|err| err.to_string())?;
        self.keep_admission_index(index)
    }

    fn keep_admission_index(&mut self, index: AdmissionIndex) -> Result<(), String> {
        if self.admission_index.as_ref().is_some_and(|held| index.sequence <= held.sequence) {
            return Ok(());
        }
        let bytes = index.encode().map_err(|err| err.to_string())?;
        atomic(&self.dir.join("admission-index"), &bytes)?;
        self.admission_index = Some(index);
        Ok(())
    }

    /// A listing hash stays when this node has no index for the store, or the
    /// index names a different store record. When the digest matches, a hash
    /// the index leaves out is a retraction.
    fn index_allows(&self, store_id: &[u8; 32], hash: &[u8; 32]) -> bool {
        let Some(index) = self.catalog_indexes.get(store_id) else {
            return true;
        };
        let Some(store) = self.stores.get(store_id) else {
            return true;
        };
        let Ok(digest) = store.record_digest() else {
            return true;
        };
        if index.record_digest != digest {
            return true;
        }
        index.listings.binary_search(hash).is_ok()
    }

    fn index_keeps(&self, listing: &Listing) -> bool {
        let Ok(hash) = listing.content_hash() else {
            return false;
        };
        self.index_allows(listing.store_id.as_bytes(), hash.as_bytes())
    }

    fn retract_absent(&mut self, index: &CatalogIndex) {
        let Some(store) = self.stores.get(&index.store_id) else { return };
        let Ok(digest) = store.record_digest() else { return };
        if digest != index.record_digest {
            return;
        }
        let stale: Vec<[u8; 32]> = self
            .listings
            .iter()
            .filter(|(hash, listing)| listing.store_id.as_bytes() == &index.store_id && index.listings.binary_search(hash).is_err())
            .map(|(hash, _)| *hash)
            .collect();
        for hash in stale {
            self.listings.remove(&hash);
            let _ = fs::remove_file(self.listing_path(&hash));
        }
    }

    #[cfg(test)]
    pub(crate) fn accept_document(&mut self, bytes: &[u8], key: &[u8; 32]) -> Result<u64, String> {
        self.accept_document_at(bytes, key, None)
    }

    /// Keep a higher sequence signed by `key` whose fee terms extend the held ones.
    pub(crate) fn accept_document_at(&mut self, bytes: &[u8], key: &[u8; 32], tip: Option<u32>) -> Result<u64, String> {
        let document = SeedDocument::decode(bytes).map_err(|err| err.to_string())?;
        document.verify_signer(key).map_err(|err| err.to_string())?;
        self.heads.accept(document, tip)
    }

    /// The signed name is shown when this node follows the id, the admission
    /// still passes `live`, or the directory snapshot names the id and no
    /// admission on file failed `live`. A missing record is not a shop. The
    /// own id is shown from this node's catalog, not here.
    pub(crate) fn show_store(&self, id: &[u8; 32], live: &dyn Fn(&Admission) -> bool) -> bool {
        if !self.stores.contains_key(id) {
            return false;
        }
        if self.followed.contains(id) {
            return true;
        }
        if let Some(admission) = self.admissions.get(id) {
            return live(admission);
        }
        self.snapshot_names(id)
    }

    pub(crate) fn listing_wanted(&self, id: &[u8; 32], own: &[u8; 32]) -> bool {
        id == own
            || self.followed.contains(id)
            || self.opened.iter().rev().take(OPENED_CAP).any(|open| open == id)
            || self.admissions.contains_key(id)
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
                || (show(listing.store_id.as_bytes())
                    && self.listing_wanted(listing.store_id.as_bytes(), own.as_bytes())
                    && self.index_keeps(listing))
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
            if !show(listing.store_id.as_bytes()) || !self.listing_wanted(listing.store_id.as_bytes(), own.as_bytes()) || !self.index_keeps(listing) {
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
        if let Ok(bytes) = fs::read(self.dir.join("admission-index")) {
            if let Ok(index) = AdmissionIndex::decode(&bytes) {
                if index.verify().is_ok() {
                    self.admission_index = Some(index);
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.dir.join("indexes")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else { continue };
                let Ok(index) = CatalogIndex::decode(&bytes) else { continue };
                if index.verify().is_err() {
                    continue;
                }
                if self.catalog_indexes.get(&index.store_id).is_some_and(|held| index.sequence <= held.sequence) {
                    continue;
                }
                self.catalog_indexes.insert(index.store_id, index);
            }
        }
        let indexes: Vec<CatalogIndex> = self.catalog_indexes.values().cloned().collect();
        for index in indexes {
            self.retract_absent(&index);
        }
        Ok(())
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

    fn index_path(&self, id: &[u8; 32]) -> PathBuf {
        self.dir.join("indexes").join(hex(id))
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
        self.heads.seed_bytes().ok_or_else(|| "no seed document".to_string())
    }

    fn head_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.heads.head_bytes())
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

    fn catalog_index_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        match self.catalog_indexes.get(id) {
            Some(index) => Ok(Some(index.encode().map_err(|err| err.to_string())?)),
            None => Ok(None),
        }
    }

    fn admission_index_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        match &self.admission_index {
            Some(index) => Ok(Some(index.encode().map_err(|err| err.to_string())?)),
            None => Ok(None),
        }
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
fn pull_into(held: &impl Held, sources: &[&dyn Source], key: &[u8; 32], own: [u8; 32], tip: Option<u32>) -> Result<(), String> {
    let sources = &sources[..sources.len().min(PASS_ATTEMPTS)];
    pull_head(held, sources, key, tip)?;
    pull_admissions(held, sources)?;
    pull_admission_index(held, sources, key)?;
    pull_stores(held, sources)?;
    pull_catalog_indexes(held, sources, own)?;
    pull_wanted_listings(held, sources, own)
}

/// Ask each source for its version 3 head, or its version 2 head when it serves none.
fn pull_head(held: &impl Held, sources: &[&dyn Source], key: &[u8; 32], tip: Option<u32>) -> Result<(), String> {
    let fetched = sources
        .iter()
        .filter_map(|source| match source.head_bytes() {
            Ok(Some(bytes)) => Some(bytes),
            Ok(None) => source.seed_bytes().ok(),
            Err(_) => None,
        })
        .filter_map(|bytes| SeedDocument::decode(&bytes).ok())
        .filter(|document| document.verify_signer(key).is_ok())
        .collect();
    held.with(|replica| replica.heads.keep(fetched, tip))
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

fn pull_admission_index(held: &impl Held, sources: &[&dyn Source], key: &[u8; 32]) -> Result<(), String> {
    let mut best: Option<AdmissionIndex> = None;
    for source in sources {
        let Ok(Some(bytes)) = source.admission_index_bytes() else { continue };
        let Ok(index) = AdmissionIndex::decode(&bytes) else { continue };
        if index.verify_signer(key).is_err() {
            continue;
        }
        if best.as_ref().is_none_or(|held_index| index.sequence > held_index.sequence) {
            best = Some(index);
        }
    }
    if let Some(index) = best {
        held.with(|replica| replica.keep_admission_index(index))?;
    }
    Ok(())
}

fn pull_catalog_indexes(held: &impl Held, sources: &[&dyn Source], own: [u8; 32]) -> Result<(), String> {
    let ids = held.with(|replica| replica.listing_ids(own));
    for id in ids {
        let mut best: Option<CatalogIndex> = None;
        for source in sources {
            let Ok(Some(bytes)) = source.catalog_index_bytes(&id) else { continue };
            let Ok(index) = CatalogIndex::decode(&bytes) else { continue };
            if index.store_id != id || index.verify().is_err() {
                continue;
            }
            if best.as_ref().is_none_or(|held_index| index.sequence > held_index.sequence) {
                best = Some(index);
            }
        }
        if let Some(index) = best {
            held.with(|replica| replica.keep_catalog_index(index))?;
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
            tormail::QUERY_SEED | tormail::QUERY_HEAD => from_replica,
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
            tormail::QUERY_ADMISSIONS | tormail::QUERY_INBOX | tormail::QUERY_CATALOG_INDEX | tormail::QUERY_ADMISSION_INDEX => from_replica,
            _ => None,
        }
    }

    /// Configure the directory operator secret key for automatic `OBAX`
    /// admission snapshot signing. Refuses a key that does not match [`DOCUMENT_KEY`].
    pub fn set_admission_signer(&self, secret_bytes: [u8; 32]) -> Result<(), String> {
        let secret = SecretKey::from_slice(&secret_bytes)
            .map_err(|_| "the admission signing key is not a valid secret key".to_string())?;
        if secret.x_only_public_key(&Secp256k1::new()).0.serialize() != DOCUMENT_KEY {
            return Err("the admission signing key does not match the directory key shipped in this binary".into());
        }
        *self.admission_signer.lock().unwrap_or_else(|err| err.into_inner()) = Some(secret);
        Ok(())
    }

    pub(crate) fn has_admission_signer(&self) -> bool {
        self.admission_signer.lock().unwrap_or_else(|err| err.into_inner()).is_some()
    }

    /// SOCKS5 proxy for outbound directory queries and submissions, when the
    /// node runs with `--proxy` or `--external-tor`.
    pub(crate) fn directory_proxy(&self) -> Option<String> {
        self.local.egress.proxy.clone().or_else(|| {
            self.remote
                .uses_external_tor()
                .then(|| format!("socks5h://{}", crate::remote::external_tor::DEFAULT_SOCKS))
        })
    }

    fn own_onions(&self) -> Vec<String> {
        let mut onions = Vec::new();
        if let Some(onion) = self.advertised_onion() {
            onions.push(onion);
        }
        if let Some(onion) = self.remote.services.sale_onion() {
            if !onions.iter().any(|existing| existing == &onion) {
                onions.push(onion);
            }
        }
        onions
    }

    fn is_introduction(&self) -> bool {
        let own = self.own_onions();
        if own.is_empty() {
            return false;
        }
        let hosts = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).introduction_hosts();
        hosts.iter().any(|host| own.iter().any(|mine| mine == host))
    }

    /// Verify all admissions on-chain and, if the set of live admitted stores
    /// changed (or no `OBAX` snapshot exists yet for the current head), sign
    /// and atomically persist a new `AdmissionIndex` (`OBAX`) with a monotonic
    /// sequence number. Returns `Ok(Some((sequence, encoded_bytes)))` when a
    /// new snapshot was issued.
    pub(crate) fn try_issue_admission_index(&self) -> Result<Option<(u64, Vec<u8>)>, String> {
        let secret = *self.admission_signer.lock().unwrap_or_else(|err| err.into_inner());
        let Some(secret) = secret else {
            return Ok(None);
        };
        self.try_issue_admission_index_with(&secret, &DOCUMENT_KEY)
    }

    pub(crate) fn try_issue_admission_index_with(
        &self,
        secret: &SecretKey,
        expected_key: &[u8; 32],
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        if secret.x_only_public_key(&Secp256k1::new()).0.serialize() != *expected_key {
            return Err("admission signer does not match expected directory key".into());
        }
        let (head, admissions, existing_snapshot_stores) = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            let head = replica.head();
            let admissions = replica.admissions();
            let existing = replica.current_snapshot().map(|snap| snap.stores.clone());
            (head, admissions, existing)
        };
        let Some(head) = head else {
            return Ok(None);
        };
        if head.signer != *expected_key {
            return Ok(None);
        }
        // Do not wipe a pre-existing non-empty snapshot before any admission
        // records have been pulled, and do not sign an empty snapshot when no
        // admissions have ever been seen.
        if admissions.is_empty() {
            return Ok(None);
        }
        let live_set = crate::admission::verified_live_admissions(
            self.chain.as_ref(),
            self.network,
            &head,
            &admissions,
        )?;
        let mut live_stores: Vec<[u8; 32]> = live_set.iter().copied().collect();
        live_stores.sort();
        // If no existing snapshot and no live stores yet, wait until at least
        // one valid admission is confirmed before issuing seq 1.
        if existing_snapshot_stores.is_none() && live_stores.is_empty() {
            self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).set_live(live_set);
            return Ok(None);
        }
        let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        replica.set_live(live_set);
        let Some(current_head) = replica.head() else {
            return Ok(None);
        };
        if current_head.sequence != head.sequence || current_head.signer != *expected_key {
            return Ok(None);
        }
        if replica.current_snapshot().is_some_and(|snap| snap.stores == live_stores) {
            return Ok(None);
        }
        let next_seq = replica.admission_index_sequence().saturating_add(1);
        let index = AdmissionIndex::sign(
            next_seq,
            crate::own_store::today_yyyymmdd(),
            current_head.sequence,
            live_stores,
            secret,
        )
        .map_err(|err| err.to_string())?;
        let bytes = index.encode().map_err(|err| err.to_string())?;
        replica.keep_admission_index(index)?;
        Ok(Some((next_seq, bytes)))
    }

    /// Keep a signed store, listing, admission, or a higher head.
    pub(crate) fn directory_submit(&self, kind: u8, body: &[u8]) -> bool {
        let tip = if kind == tormail::SUBMIT_DOCUMENT { self.chain.tip_height().ok() } else { None };
        let accepted = {
            let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            match kind {
                tormail::SUBMIT_STORE => replica.accept_store(body).map(|_| ()),
                tormail::SUBMIT_LISTING => replica.accept_listing(body),
                tormail::SUBMIT_DOCUMENT => replica.accept_document_at(body, &DOCUMENT_KEY, tip).map(|_| ()),
                tormail::SUBMIT_ADMISSION => replica.accept_admission(body).map(|_| ()),
                tormail::SUBMIT_CATALOG_INDEX => replica.accept_catalog_index(body),
                tormail::SUBMIT_ADMISSION_INDEX => replica.accept_admission_index(body, &DOCUMENT_KEY),
                _ => Err("unknown directory submission".into()),
            }
        };
        if accepted.is_ok() {
            self.recheck_admissions();
            if matches!(kind, tormail::SUBMIT_ADMISSION | tormail::SUBMIT_DOCUMENT) {
                match self.try_issue_admission_index() {
                    Ok(Some((seq, bytes))) => {
                        eprintln!("ob-node: issued OBAX seq {seq}");
                        self.spawn_propagate_admission_index(seq, bytes);
                    }
                    Ok(None) => {}
                    Err(err) => eprintln!("ob-node: automatic OBAX issuance skipped: {err}"),
                }
            }
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
        // Stores followed by URL are served from `replicas.rs`; the directory
        // replica does not know them, so `apply` drops any it has no record
        // of. Those are put back. A store both sources hold keeps the
        // directory's handling, as before. Copied before the other locks,
        // which `replicate` takes in the other order.
        let followed_by_url = self.replicas().served_copies();
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let own = self.own_store;
        let mut catalog = self.catalog.write().unwrap_or_else(|err| err.into_inner());
        replica.apply(
            &mut catalog,
            own,
            |store| store.id == own || crate::demo_secret(store).is_some(),
            |admission| replica.is_live(admission),
        );
        for copy in &followed_by_url {
            if !catalog.stores.iter().any(|store| store.id == copy.store.id) {
                crate::replicas::serve_replica(&mut catalog, copy);
            }
        }
    }

    /// Listing hashes in the catalog index when that index names this store's
    /// current record. `None` when there is no index, or the index names a
    /// different record: those listings are not a retraction.
    pub(crate) fn catalog_index_hashes(&self, store: &Store) -> Option<Vec<String>> {
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let index = replica.catalog_indexes.get(store.id.as_bytes())?;
        let digest = store.record_digest().ok()?;
        if index.record_digest != digest {
            return None;
        }
        Some(index.listings.iter().map(|hash| hex(hash)).collect())
    }

    /// The own store's signed catalog index, when it names the record served now.
    pub(crate) fn own_catalog_index_record(&self) -> Option<Vec<u8>> {
        let store = self.own_record()?;
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        let index = replica.catalog_indexes.get(store.id.as_bytes())?;
        if index.record_digest != store.record_digest().ok()? {
            return None;
        }
        index.encode().ok()
    }

    /// The signed admission snapshot, or nothing when this node is not holding one for its head.
    pub(crate) fn public_admission_record(&self) -> Option<Vec<u8>> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).admission_record()
    }

    /// Admitted stores as key and address only.
    pub(crate) fn public_admission_addresses(&self) -> Vec<(String, String)> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).admission_addresses()
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
                "admitted": replica.snapshot_names(store.id.as_bytes()),
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
            match self.try_issue_admission_index() {
                Ok(Some((seq, _))) => {
                    eprintln!("ob-node: issued OBAX seq {seq}");
                    self.refresh_market();
                }
                Ok(None) => {}
                Err(err) => eprintln!("ob-node: automatic OBAX issuance skipped: {err}"),
            }
            self.propagate_admission_index(&sources);
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
        let (store, admission, catalog_index, listings) = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            (replica.read_outbox("store"), replica.read_outbox("admission"), replica.read_outbox("catalog-index"), replica.outbox_listings())
        };
        for source in sources {
            if let Some(bytes) = &store {
                if let Err(err) = submit_record(&source.state_dir, &source.onion, source.proxy.as_deref(), tormail::SUBMIT_STORE, bytes) {
                    eprintln!("ob-node: submit store to {}: {err}", source.onion);
                }
            }
            if let Some(bytes) = &admission {
                if let Err(err) = submit_record(&source.state_dir, &source.onion, source.proxy.as_deref(), tormail::SUBMIT_ADMISSION, bytes) {
                    eprintln!("ob-node: submit admission to {}: {err}", source.onion);
                }
            }
            if let Some(bytes) = &catalog_index {
                if let Err(err) = submit_record(&source.state_dir, &source.onion, source.proxy.as_deref(), tormail::SUBMIT_CATALOG_INDEX, bytes) {
                    eprintln!("ob-node: submit catalog index to {}: {err}", source.onion);
                }
            }
        }
        for (name, bytes) in &listings {
            let already = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).listing_was_sent(name);
            if already {
                continue;
            }
            let mut kept = false;
            for source in sources {
                if submit_record(&source.state_dir, &source.onion, source.proxy.as_deref(), tormail::SUBMIT_LISTING, bytes).ok() == Some(true) {
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

    /// Push the current `OBAX` admission snapshot to peer introduction hosts
    /// that have not yet acknowledged this sequence. Runs only on nodes that
    /// sign admissions or are themselves listed as introductions.
    fn propagate_admission_index(&self, sources: &[Introduction]) {
        if !self.has_admission_signer() && !self.is_introduction() {
            return;
        }
        let snapshot = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.current_snapshot().and_then(|snap| {
                snap.encode().ok().map(|bytes| (snap.sequence, bytes))
            })
        };
        let Some((seq, bytes)) = snapshot else {
            return;
        };
        for source in sources {
            let already = self
                .admission_index_pushed
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .get(&source.onion)
                .copied()
                .unwrap_or(0);
            if already >= seq {
                continue;
            }
            match submit_record(
                &source.state_dir,
                &source.onion,
                source.proxy.as_deref(),
                tormail::SUBMIT_ADMISSION_INDEX,
                &bytes,
            ) {
                Ok(true) => {
                    self.admission_index_pushed
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .insert(source.onion.clone(), seq);
                    eprintln!("ob-node: propagated OBAX seq {seq} to {}", source.onion);
                }
                Ok(false) => {
                    eprintln!("ob-node: {} rejected OBAX seq {seq}", source.onion);
                }
                Err(err) => {
                    eprintln!("ob-node: propagate OBAX seq {seq} to {}: {err}", source.onion);
                }
            }
        }
    }

    /// Best-effort asynchronous push of a newly issued `OBAX` snapshot to all
    /// peer introductions so they receive it without waiting for the next
    /// 60-second pass and without blocking the incoming `OBSB` handler.
    fn spawn_propagate_admission_index(&self, seq: u64, bytes: Vec<u8>) {
        if !self.local.introductions {
            return;
        }
        let own = self.own_onions();
        let peers: Vec<String> = self
            .seed_replica
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .introduction_hosts()
            .into_iter()
            .filter(|onion| !own.iter().any(|mine| mine == onion))
            .collect();
        if peers.is_empty() {
            return;
        }
        let state_dir = self.data_dir.join("tor-client");
        let proxy = self.directory_proxy();
        thread::spawn(move || {
            for onion in peers {
                match submit_record(&state_dir, &onion, proxy.as_deref(), tormail::SUBMIT_ADMISSION_INDEX, &bytes) {
                    Ok(true) => eprintln!("ob-node: propagated OBAX seq {seq} to {onion}"),
                    Ok(false) => eprintln!("ob-node: {onion} rejected OBAX seq {seq}"),
                    Err(err) => eprintln!("ob-node: propagate OBAX seq {seq} to {onion}: {err}"),
                }
            }
        });
    }

    pub(crate) fn pull_directory(&self, sources: &[&dyn Source]) -> Result<(), String> {
        let own = *self.own_store.as_bytes();
        let tip = self.chain.tip_height().ok();
        pull_into(&self.seed_replica, sources, &DOCUMENT_KEY, own, tip)?;
        self.recheck_admissions();
        self.refresh_market();
        Ok(())
    }

    /// Follow a store. A record this node already holds goes back in the shop
    /// now, so a store unfollowed and followed again opens without a 404.
    pub(crate) fn follow_store(&self, id: [u8; 32]) -> Result<(), String> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).follow(id)?;
        self.refresh_market();
        Ok(())
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
        let proxy = self.directory_proxy();
        let primary = match onion.as_deref() {
            Some(onion) if !onion.is_empty() => match ask(&state_dir, onion, proxy.as_deref(), tormail::QUERY_LISTINGS, id) {
                Ok(Some(blob)) => split_listings(&blob),
                Ok(None) => Ok(Vec::new()),
                Err(err) => Err(err),
            },
            _ => Err("the store record has no onion".into()),
        };
        let fallback = self.introduction_listings(id);
        let lists = listings_after_dial(primary, fallback);
        self.keep_opened_index(id, onion.as_deref());
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).keep_listings(&id, &lists)?;
        self.refresh_market();
        Ok(())
    }

    /// Take a catalog index from the store onion, or from one introduction when
    /// that answer does not verify. A miss leaves the listings unchecked.
    fn keep_opened_index(&self, id: [u8; 32], onion: Option<&str>) {
        let proxy = self.directory_proxy();
        let from_store = match onion {
            Some(onion) if !onion.is_empty() => {
                let state_dir = self.data_dir.join("tor-client");
                ask(&state_dir, onion, proxy.as_deref(), tormail::QUERY_CATALOG_INDEX, id).ok().flatten()
            }
            _ => None,
        };
        if from_store.as_ref().is_some_and(|bytes| {
            self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).accept_catalog_index(bytes).is_ok()
        }) {
            return;
        }
        let Some(bytes) = self.introduction_index(id) else { return };
        let _ = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).accept_catalog_index(&bytes);
    }

    fn introduction_index(&self, id: [u8; 32]) -> Option<Vec<u8>> {
        if !self.local.introductions {
            return None;
        }
        let hosts = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).introduction_hosts();
        let state_dir = self.data_dir.join("tor-client");
        let proxy = self.directory_proxy();
        for onion in hosts.iter().take(PASS_ATTEMPTS) {
            if let Ok(Some(bytes)) = ask(&state_dir, onion, proxy.as_deref(), tormail::QUERY_CATALOG_INDEX, id) {
                return Some(bytes);
            }
        }
        None
    }

    fn introduction_listings(&self, id: [u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        if !self.local.introductions {
            return Err("introductions are off".into());
        }
        let hosts = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).introduction_hosts();
        let state_dir = self.data_dir.join("tor-client");
        let proxy = self.directory_proxy();
        let mut last = String::from("no introduction answered");
        for onion in hosts.iter().take(PASS_ATTEMPTS) {
            match ask(&state_dir, onion, proxy.as_deref(), tormail::QUERY_LISTINGS, id) {
                Ok(Some(blob)) => return split_listings(&blob),
                Ok(None) => return Ok(Vec::new()),
                Err(err) => last = err,
            }
        }
        Err(last)
    }

    fn pass_introductions(&self, origin: usize) -> Vec<Introduction> {
        let state_dir = self.data_dir.join("tor-client");
        let proxy = self.directory_proxy();
        let own = self.own_onions();
        let hosts: Vec<String> = self
            .seed_replica
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .introduction_hosts()
            .into_iter()
            .filter(|onion| !own.iter().any(|mine| mine == onion))
            .collect();
        crate::seeds::pass_hosts(&hosts, origin)
            .into_iter()
            .map(|onion| Introduction { onion, state_dir: state_dir.clone(), proxy: proxy.clone() })
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

    /// Sign the own store's catalog index and queue it for the introductions.
    /// The catalog lock is not held across the replica lock. An unchanged
    /// catalog keeps its sequence.
    pub(crate) fn queue_catalog_index(&self) -> Result<(), String> {
        let Some(secret) = self.own_signing_key() else { return Ok(()) };
        let (store, listings) = {
            let catalog = self.catalog();
            let Some(store) = catalog.store(&self.own_store).cloned() else { return Ok(()) };
            let listings = catalog.listings.iter().filter(|listing| listing.store_id == store.id).cloned().collect::<Vec<_>>();
            (store, listings)
        };
        let digest = store.record_digest().map_err(|err| err.to_string())?;
        let mut hashes = Vec::with_capacity(listings.len());
        for listing in &listings {
            hashes.push(*listing.content_hash().map_err(|err| err.to_string())?.as_bytes());
        }
        let mut sorted = hashes.clone();
        sorted.sort();
        let mut replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        if replica.catalog_indexes.get(store.id.as_bytes()).is_some_and(|index| index.record_digest == digest && index.listings == sorted) {
            return Ok(());
        }
        let sequence = replica.catalog_indexes.get(store.id.as_bytes()).map(|index| index.sequence.saturating_add(1)).unwrap_or(1);
        let index = CatalogIndex::sign(sequence, crate::own_store::today_yyyymmdd(), digest, hashes, &secret).map_err(|err| err.to_string())?;
        for listing in &listings {
            let hash = *listing.content_hash().map_err(|err| err.to_string())?.as_bytes();
            replica.save_listing(hash, listing)?;
            let name = format!("listing-{}", hex(&hash));
            if replica.read_outbox(&name).is_none() {
                replica.write_outbox(&name, &listing.encode().map_err(|err| err.to_string())?)?;
            }
        }
        replica.save_store(&store)?;
        replica.queue_store(&store)?;
        let bytes = index.encode().map_err(|err| err.to_string())?;
        replica.write_outbox("catalog-index", &bytes)?;
        replica.keep_catalog_index(index)
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
        self.queue_catalog_index().map_err(OrderError::Failed)?;
        listing_value(&self.catalog(), &listing).map_err(|err| OrderError::Failed(err.to_string()))
    }
}

/// Ask `onions` for the directory head and keep the highest sequence that
/// verifies under the shipped key and extends the held terms. `dir` is one
/// network's data directory (`…/testnet4`). No chain is asked, so a head that
/// drops terms is not taken here.
pub(crate) fn pull_onions(dir: &Path, onions: &[String]) -> Result<(u64, Vec<String>), String> {
    let mut replica = Replica::open(dir).map_err(|err| err.to_string())?;
    let state_dir = dir.join("tor-client");
    let sources: Vec<Introduction> = onions
        .iter()
        .map(|onion| Introduction { onion: onion.clone(), state_dir: state_dir.clone(), proxy: None })
        .collect();
    let reported: Vec<Report<'_>> = sources.iter().map(|source| Report { inner: source }).collect();
    let refs: Vec<&dyn Source> = reported.iter().map(|source| source as &dyn Source).collect();
    replica.pull(&refs, &DOCUMENT_KEY, [0u8; 32])?;
    let document = replica.head().ok_or_else(|| "no introduction returned a document".to_string())?;
    let mut lines = crate::head_plan::describe_head(&document);
    if let Some((sequence, reason)) = replica.head_refused() {
        lines.push(format!("refused sequence {sequence}: {reason}"));
    }
    Ok((document.sequence, lines))
}

/// Read `(head_sequence, next_snapshot_sequence, admitted_store_ids)` from the
/// directory replica stored in `dir` (`…/testnet4`).
pub(crate) fn replica_admission_state(dir: &Path) -> Result<(u64, u64, Vec<[u8; 32]>), String> {
    Replica::open(dir).map_err(|err| err.to_string())?.admission_state()
}

/// Submit a signed admission snapshot (`OBAX`) to each introduction in `onions`.
pub(crate) fn submit_admission_index(dir: &Path, onions: &[String], bytes: &[u8]) -> Result<Vec<(String, bool)>, String> {
    let index = AdmissionIndex::decode(bytes).map_err(|err| err.to_string())?;
    index.verify_signer(&DOCUMENT_KEY).map_err(|err| err.to_string())?;
    let state_dir = dir.join("tor-client");
    let mut results = Vec::with_capacity(onions.len());
    for onion in onions {
        let kept = submit_record(&state_dir, onion, None, tormail::SUBMIT_ADMISSION_INDEX, bytes)?;
        results.push((onion.clone(), kept));
    }
    Ok(results)
}

struct Report<'a> {
    inner: &'a Introduction,
}

impl Source for Report<'_> {
    fn seed_bytes(&self) -> Result<Vec<u8>, String> {
        self.inner.seed_bytes().inspect_err(|err| eprintln!("ob-node: introduction {}: {err}", self.inner.onion))
    }

    fn head_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        self.inner.head_bytes().inspect_err(|err| eprintln!("ob-node: introduction {}: {err}", self.inner.onion))
    }

    fn store_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        self.inner.store_bytes(id)
    }

    fn listing_bytes(&self, id: &[u8; 32]) -> Result<Vec<Vec<u8>>, String> {
        self.inner.listing_bytes(id)
    }

    fn admission_bytes(&self, cursor: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        self.inner.admission_bytes(cursor)
    }

    fn catalog_index_bytes(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, String> {
        self.inner.catalog_index_bytes(id)
    }

    fn admission_index_bytes(&self) -> Result<Option<Vec<u8>>, String> {
        self.inner.admission_index_bytes()
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
        assert_eq!(peer.head().unwrap().sequence, 5);
        assert_eq!(peer.stores.len(), 1);
        assert!(!peer.listings.is_empty());

        peer.pull(&[&low, &stopped], &key, [0u8; 32]).unwrap();
        assert_eq!(peer.head().unwrap().sequence, 5, "an older document and a stopped host do not replace it");
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
        let b_path = format!("/stores/{}", hex(&b_store));
        a.unfollow_store(b_store).unwrap();
        assert!(!a.catalog().stores.iter().any(|store| store.id.as_bytes() == &b_store), "an unfollowed store leaves the shop");
        let (status, _) = harness::request(&a, "GET", &b_path, "");
        assert!(status.contains("404"), "{status}");
        a.follow_store(b_store).unwrap();
        assert!(a.catalog().stores.iter().any(|store| store.id.as_bytes() == &b_store), "following again puts the held record back at once");
        assert!(a.catalog().listings.iter().any(|item| item.title == "Maple spoon"));
        let (status, value) = harness::request(&a, "GET", &b_path, "");
        assert!(status.contains("200"), "{status} {value}");
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
        assert_eq!(peer.head().unwrap().sequence, 1);

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
        assert_eq!(peer.head().unwrap().sequence, 1, "admitting a store does not change the head sequence");
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
        assert_eq!(fresh.head().unwrap().sequence, 2, "one of three introductions down still returns the head");
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
        assert!(held.contains(store.id.as_bytes()), "an admitted store now has its listings pulled");
        let live = |_: &Admission| true;
        let (names, titles) = peer.search("Cedar", &own, &live);
        assert_eq!(names, vec!["Cedar Workshop".to_string()]);
        assert_eq!(titles, vec!["Cedar spoon".to_string()], "admitted stores' listings are searchable");
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
            source.listings.borrow().contains(&id),
            "admitted stores now have their listings pulled"
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
        let send = |output: Vec<TxOut>| {
            let tx = Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::default(),
                    script_sig: bitcoin::ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::default(),
                }],
                output,
            };
            chain.broadcast(&tx).unwrap();
            chain.mine();
            tx
        };
        let pay = |sats: u64, id: &[u8; 32]| {
            let script = crate::admission::store_fee_address(&key, id, NetworkMode::Testnet4.bitcoin()).unwrap().script_pubkey();
            send(vec![TxOut { value: Amount::from_sat(sats), script_pubkey: script }])
        };
        assert_eq!(page_cap(), tormail::MAX_QUERY_LEN / ADMISSION_LEN);
        assert_eq!(20_000 * ADMISSION_LEN, 2_820_000);
        assert_eq!(20_000_usize.div_ceil(page_cap()), 3);
        let tx = pay(FEE_SATS, &store_id);
        assert!(tx.output.iter().any(|output| output.value.to_sat() >= FEE_SATS));
        assert_eq!(tx.output.len(), 1, "the fee transaction is one payment to the store's fee address, not an order");
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
        let legacy = send(vec![
            TxOut { value: Amount::from_sat(FEE_SATS), script_pubkey: crate::admission::fee_script(&key, NetworkMode::Testnet4.bitcoin()).unwrap() },
            TxOut { value: Amount::ZERO, script_pubkey: crate::admission::fee_return(&store_id).unwrap() },
        ]);
        let legacy_txid = legacy.compute_txid().to_byte_array();
        let v1 = Admission::sign_v1(store_id, legacy_txid, 0, chain.tip(), &secret).unwrap();
        assert!(crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &v1), "a version 1 admission still lists its store");
        let v2_on_legacy = Admission::sign(store_id, legacy_txid, 0, chain.tip(), &secret).unwrap();
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &v2_on_legacy), "version 2 needs the store's own address");
        let v1_on_store_address = Admission::sign_v1(store_id, tx.compute_txid().to_byte_array(), 0, height, &secret).unwrap();
        assert!(!crate::admission::fee_confirmed(&chain, NetworkMode::Testnet4, &head, &v1_on_store_address), "version 1 needs the head's fee key and the OP_RETURN");

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
        assert_eq!(catalog.listings.len(), 1, "admitted stores' listings appear in the catalog");
        let hidden = TempDir::new();
        let mut bare = Replica::open(&hidden.0).unwrap();
        bare.accept_document(&head.encode().unwrap(), &key).unwrap();
        bare.accept_admission(&admission.encode().unwrap()).unwrap();
        let mut empty = Catalog { stores: vec![], moderator: catalog.moderator.clone(), listings: vec![] };
        bare.apply(&mut empty, StoreId::from_bytes([1u8; 32]), |_| false, &live);
        assert!(empty.stores.is_empty(), "an admission with no store record is not a shop");
        *chain.0.tip.lock().unwrap() = crate::admission::listed_until(&head.schedule, height).saturating_add(1);
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
        let address = crate::admission::store_fee_address(&xonly(&secret), node.own_store.as_bytes(), NetworkMode::Testnet4.bitcoin()).unwrap();
        assert!(tx.output.iter().any(|output| output.script_pubkey == address.script_pubkey() && output.value.to_sat() >= FEE_SATS));
        assert!(!tx.output.iter().any(|output| output.script_pubkey.is_op_return()), "the store's fee address names it; no OP_RETURN");
        node.publish_admission().unwrap();
        assert_eq!(chain.broadcasts().len(), 1, "a fee in the mempool is not paid again");
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

    fn shop(label: &str, name: &str) -> (bitcoin::secp256k1::SecretKey, openbazaar_protocol::Store) {
        let secret = secret_from_label(label);
        let store = openbazaar_protocol::Store::sign(
            name,
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: format!("{}.onion", "d".repeat(56)),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile { created_on: 20261123, ..openbazaar_protocol::StoreProfile::default() },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        (secret, store)
    }

    fn goods(store: &openbazaar_protocol::Store, secret: &bitcoin::secp256k1::SecretKey, title: &str, price: u64) -> Listing {
        Listing::sign(
            store.id, title, "Home", price, "Maine", "A small spoon.", store.name.clone(), 3, 20261123, false, ListingKind::Digital, "", 0, 0,
            ContentLabel::GENERAL, secret,
        )
        .unwrap()
    }

    #[test]
    fn a_snapshot_names_a_store_the_peer_was_not_following() {
        let dir_secret = secret_from_label("snapshot-directory");
        let dir_key = xonly(&dir_secret);
        let (_, store) = shop("snapshot-shop", "Cedar Workshop");
        let (_, hidden) = shop("snapshot-hidden", "Hidden Workshop");
        let head = SeedDocument::sign(4, vec![onion('a')], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let snapshot = AdmissionIndex::sign(1, 20261123, 4, vec![*store.id.as_bytes()], &dir_secret).unwrap();
        let intro_dir = TempDir::new();
        let mut intro = Replica::open(&intro_dir.0).unwrap();
        intro.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
        intro.accept_store(&store.encode().unwrap()).unwrap();
        intro.accept_store(&hidden.encode().unwrap()).unwrap();
        intro.accept_admission_index(&snapshot.encode().unwrap(), &dir_key).unwrap();
        let mut forged = snapshot.encode().unwrap();
        let last = forged.len() - 1;
        forged[last] ^= 0xff;
        assert!(intro.accept_admission_index(&forged, &dir_key).is_err(), "a bad snapshot signature is dropped");

        let peer_dir = TempDir::new();
        let mut peer = Replica::open(&peer_dir.0).unwrap();
        peer.pull(&[&intro], &dir_key, [0u8; 32]).unwrap();
        assert!(peer.stores.contains_key(store.id.as_bytes()), "the snapshot's store is fetched");
        assert!(!peer.followed.contains(store.id.as_bytes()));
        assert!(!peer.stores.contains_key(hidden.id.as_bytes()), "a store the snapshot does not name is not fetched");
        let refuse = |_: &Admission| false;
        let allow = |_: &Admission| true;
        assert!(peer.show_store(store.id.as_bytes(), &refuse));

        let admission = Admission::sign(*store.id.as_bytes(), [7u8; 32], 0, 50, &secret_from_label("snapshot-shop")).unwrap();
        intro.accept_admission(&admission.encode().unwrap()).unwrap();
        peer.pull(&[&intro], &dir_key, [0u8; 32]).unwrap();
        assert!(!peer.show_store(store.id.as_bytes(), &refuse), "a failed fee check drops the store the snapshot named");
        assert!(peer.show_store(store.id.as_bytes(), &allow));
    }

    #[test]
    fn a_catalog_index_drops_a_listing_it_does_not_name() {
        let (secret, store) = shop("index-shop", "Cedar Workshop");
        let kept = goods(&store, &secret, "Cedar spoon", 2_400);
        let dropped = goods(&store, &secret, "Maple spoon", 2_600);
        let kept_hash = *kept.content_hash().unwrap().as_bytes();
        let digest = store.record_digest().unwrap();
        let intro_dir = TempDir::new();
        let mut intro = Replica::open(&intro_dir.0).unwrap();
        intro.accept_store(&store.encode().unwrap()).unwrap();
        intro.accept_listing(&kept.encode().unwrap()).unwrap();
        intro.accept_listing(&dropped.encode().unwrap()).unwrap();
        let mismatch = CatalogIndex::sign(1, 20261123, [9u8; 32], vec![kept_hash], &secret).unwrap();
        intro.accept_catalog_index(&mismatch.encode().unwrap()).unwrap();
        let loose_dir = TempDir::new();
        let mut loose = Replica::open(&loose_dir.0).unwrap();
        loose.follow(*store.id.as_bytes()).unwrap();
        loose.pull(&[&intro], &[0u8; 32], [0u8; 32]).unwrap();
        assert_eq!(loose.listings.len(), 2, "an index for a different record does not drop listings");

        let matching = CatalogIndex::sign(2, 20261123, digest, vec![kept_hash], &secret).unwrap();
        intro.accept_catalog_index(&matching.encode().unwrap()).unwrap();
        let mut bad = matching.encode().unwrap();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(intro.accept_catalog_index(&bad).is_err());
        let older = CatalogIndex::sign(1, 20261123, digest, vec![kept_hash, *dropped.content_hash().unwrap().as_bytes()], &secret).unwrap();
        intro.accept_catalog_index(&older.encode().unwrap()).unwrap();
        assert_eq!(intro.catalog_indexes.get(store.id.as_bytes()).unwrap().sequence, 2);

        let peer_dir = TempDir::new();
        let mut peer = Replica::open(&peer_dir.0).unwrap();
        peer.follow(*store.id.as_bytes()).unwrap();
        peer.pull(&[&intro], &[0u8; 32], [0u8; 32]).unwrap();
        peer.keep_listings(store.id.as_bytes(), &[kept.encode().unwrap(), dropped.encode().unwrap()]).unwrap();
        let titles: Vec<_> = peer.listings.values().map(|listing| listing.title.clone()).collect();
        assert_eq!(titles, vec!["Cedar spoon".to_string()]);
    }

    #[test]
    fn publishing_a_listing_signs_the_catalog_index_and_the_directory_list_marks_the_snapshot() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let body = r#"{"title":"Cedar spoon","category":"Home","shipFrom":"Maine","description":"A small cedar spoon.","priceSats":2400,"stock":3}"#;
        let (status, value) = harness::request(&node, "POST", "/listings", body);
        assert!(status.contains("200"), "{status} {value}");
        let bytes = node.seed_replica.lock().unwrap().read_outbox("catalog-index").expect("the catalog index is queued");
        let index = CatalogIndex::decode(&bytes).unwrap();
        index.verify().unwrap();
        assert_eq!((index.store_id, index.sequence), (*node.own_store.as_bytes(), 1));
        assert_eq!(index.record_digest, node.own_record().unwrap().record_digest().unwrap());
        let hash = node.catalog().listings.iter().find(|listing| listing.title == "Cedar spoon").unwrap().content_hash().unwrap();
        assert!(index.listings.contains(hash.as_bytes()));

        let dir_secret = secret_from_label("listed-directory");
        let dir_key = xonly(&dir_secret);
        let (_, store) = shop("listed-shop", "Cedar Workshop");
        let head = SeedDocument::sign(4, vec![onion('a')], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let snapshot = AdmissionIndex::sign(1, 20261123, 4, vec![*store.id.as_bytes()], &dir_secret).unwrap();
        {
            let mut replica = node.seed_replica.lock().unwrap();
            replica.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
            replica.accept_store(&store.encode().unwrap()).unwrap();
            replica.accept_admission_index(&snapshot.encode().unwrap(), &dir_key).unwrap();
        }
        let value = node.directory_value();
        let row = value["stores"].as_array().unwrap().iter().find(|row| row["name"] == "Cedar Workshop").unwrap();
        assert_eq!((row["admitted"].as_bool(), row["added"].as_bool()), (Some(true), Some(false)));
    }

    #[test]
    fn opening_an_admitted_store_shows_only_listings_the_catalog_index_names() {
        use crate::harness::{self, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = harness::MockChain::new();
        let node = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let secret = secret_from_label("browse-admitted");
        let store = openbazaar_protocol::Store::sign(
            "Cedar Workshop",
            "Maine",
            openbazaar_protocol::DirectPay { bolt12_offer: String::new(), silent_payment_code: String::new() },
            openbazaar_protocol::ContentRating::General,
            openbazaar_protocol::StoreDetails {
                mailbox_key: [4u8; 32],
                onion: String::new(),
                moderators: vec![],
                profile: openbazaar_protocol::StoreProfile { created_on: 20261123, ..openbazaar_protocol::StoreProfile::default() },
                accepts_xmr: false,
                accepts_lightning: false,
                accepts_zec: false,
            },
            &secret,
        )
        .unwrap();
        let kept = goods(&store, &secret, "Cedar spoon", 2_400);
        let dropped = goods(&store, &secret, "Maple spoon", 2_600);
        let mut bad = dropped.encode().unwrap();
        let flip = bad.len() / 2;
        bad[flip] ^= 0xff;
        let dir_secret = secret_from_label("browse-directory");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(4, vec![onion('a')], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let snapshot = AdmissionIndex::sign(1, 20261123, 4, vec![*store.id.as_bytes()], &dir_secret).unwrap();
        let index = CatalogIndex::sign(1, 20261123, store.record_digest().unwrap(), vec![*kept.content_hash().unwrap().as_bytes()], &secret).unwrap();
        {
            let mut replica = node.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
            replica.accept_store(&store.encode().unwrap()).unwrap();
            replica.accept_listing(&kept.encode().unwrap()).unwrap();
            replica.accept_listing(&dropped.encode().unwrap()).unwrap();
            assert!(replica.accept_listing(&bad).is_err(), "a bad signature is dropped");
            replica.accept_catalog_index(&index.encode().unwrap()).unwrap();
            replica.accept_admission_index(&snapshot.encode().unwrap(), &dir_key).unwrap();
        }
        let key = store.id.to_hex();
        let (status, page) = harness::request(&node, "GET", &format!("/stores/{key}"), "");
        assert!(status.contains("200"), "{status} {page}");
        let titles: Vec<_> = page["listings"].as_array().unwrap().iter().map(|row| row["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles, vec!["Cedar spoon".to_string()]);
        assert_eq!(page["catalogIndex"].as_array().unwrap().len(), 1);
        let (status, catalog) = harness::request(&node, "GET", "/catalog", "");
        assert!(status.contains("200"), "{status}");
        let shown: Vec<_> = catalog["listings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["storeId"] == key)
            .map(|row| row["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(shown, vec!["Cedar spoon".to_string()]);
    }

    #[test]
    fn replica_admission_state_reads_head_next_sequence_and_admitted_stores() {
        let dir = TempDir::new();
        assert!(replica_admission_state(&dir.0).is_err(), "empty dir has no head");
        let dir_secret = secret_from_label("state-directory");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(7, vec![onion('a')], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let store_secret = secret_from_label("state-store");
        let store_id = xonly(&store_secret);
        let admission = Admission::sign(store_id, [9u8; 32], 0, 100, &store_secret).unwrap();
        let mut replica = Replica::open(&dir.0).unwrap();
        replica.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
        replica.accept_admission(&admission.encode().unwrap()).unwrap();
        assert_eq!(replica.admission_state().unwrap(), (7, 1, vec![store_id]));

        let snapshot = AdmissionIndex::sign(3, 20261123, 7, vec![store_id], &dir_secret).unwrap();
        replica.accept_admission_index(&snapshot.encode().unwrap(), &dir_key).unwrap();
        assert_eq!(replica.admission_state().unwrap(), (7, 4, vec![store_id]));
    }

    fn pay_fee_tx(chain: &crate::harness::MockChain, script: bitcoin::ScriptBuf, sats: u64, input_vout: u32) -> bitcoin::Transaction {
        use crate::chain::ChainSource;
        let tx = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint {
                    txid: bitcoin::Txid::from_raw_hash(bitcoin::hashes::Hash::all_zeros()),
                    vout: input_vout,
                },
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: bitcoin::Witness::default(),
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(sats),
                script_pubkey: script,
            }],
        };
        chain.broadcast(&tx).unwrap();
        tx
    }

    #[test]
    fn automatic_admission_verifies_fee_issues_monotonic_obax_and_rejects_invalid_or_underpaid() {
        use bitcoin::hashes::Hash;
        use openbazaar_protocol::{EPOCH_BLOCKS, FEE_SATS};
        use crate::admission::store_fee_address;
        use crate::harness::{self, MockChain, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = MockChain::new();
        let intro = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let dir_secret = secret_from_label("auto-admit-dir");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(
            1,
            vec![onion('a'), onion('b')],
            DirectoryTerms::standard(dir_key),
            None,
            &dir_secret,
        )
        .unwrap();
        intro
            .seed_replica
            .lock()
            .unwrap()
            .accept_document(&head.encode().unwrap(), &dir_key)
            .unwrap();

        let (seller_a_secret, store_a) = shop("auto-seller-a", "Cedar Works");
        let (seller_b_secret, store_b) = shop("auto-seller-b", "Birch Works");
        let id_a = *store_a.id.as_bytes();
        let id_b = *store_b.id.as_bytes();

        // 1. Underpaid fee for seller A is rejected from OBAX.
        let addr_a = store_fee_address(&dir_key, &id_a, NetworkMode::Testnet4.bitcoin()).unwrap();
        let short_tx = pay_fee_tx(&chain, addr_a.script_pubkey(), FEE_SATS - 1, 1);
        chain.mine();
        let short_admit = Admission::sign(id_a, short_tx.compute_txid().to_byte_array(), 0, chain.tip(), &seller_a_secret).unwrap();
        intro.seed_replica.lock().unwrap().accept_admission(&short_admit.encode().unwrap()).unwrap();
        assert!(
            intro.try_issue_admission_index_with(&dir_secret, &dir_key).unwrap().is_none(),
            "underpaid fee must not issue OBAX"
        );

        // 2. Unconfirmed full fee for seller B is rejected from OBAX until mined.
        let addr_b = store_fee_address(&dir_key, &id_b, NetworkMode::Testnet4.bitcoin()).unwrap();
        let tx_b = pay_fee_tx(&chain, addr_b.script_pubkey(), FEE_SATS, 2);
        let unconf_admit = Admission::sign(id_b, tx_b.compute_txid().to_byte_array(), 0, chain.tip() + 1, &seller_b_secret).unwrap();
        intro.seed_replica.lock().unwrap().accept_admission(&unconf_admit.encode().unwrap()).unwrap();
        assert!(
            intro.try_issue_admission_index_with(&dir_secret, &dir_key).unwrap().is_none(),
            "unconfirmed fee must not issue OBAX"
        );

        // 3. Full confirmed fee for seller A issues OBAX seq 1.
        let tx_a = pay_fee_tx(&chain, addr_a.script_pubkey(), FEE_SATS, 3);
        chain.mine();
        let height_a = chain.tip();
        let admit_a = Admission::sign(id_a, tx_a.compute_txid().to_byte_array(), 0, height_a, &seller_a_secret).unwrap();
        intro.seed_replica.lock().unwrap().accept_admission(&admit_a.encode().unwrap()).unwrap();
        let (seq1, bytes1) = intro
            .try_issue_admission_index_with(&dir_secret, &dir_key)
            .unwrap()
            .expect("confirmed fee issues OBAX seq 1");
        assert_eq!(seq1, 1);
        let snap1 = AdmissionIndex::decode(&bytes1).unwrap();
        snap1.verify_signer(&dir_key).unwrap();
        assert_eq!((snap1.sequence, snap1.head_sequence), (1, 1));
        // Note: mining also confirmed tx_b at height_a, whose admission already matched height_a!
        let mut expected = vec![id_a, id_b];
        expected.sort();
        assert_eq!(snap1.stores, expected);

        // 4. Idempotent when nothing changed: no redundant seq bump.
        assert!(
            intro.try_issue_admission_index_with(&dir_secret, &dir_key).unwrap().is_none(),
            "unchanged live set must not bump OBAX sequence"
        );

        // 5. Persisted atomically to disk and survives reload.
        let reloaded = Replica::open(&dir.0).unwrap();
        assert_eq!(reloaded.admission_index_sequence(), 1);
        assert_eq!(reloaded.admission_index.as_ref().unwrap().stores, expected);

        // 6. A third seller joins -> monotonic seq 2.
        let (seller_c_secret, store_c) = shop("auto-seller-c", "Maple Works");
        let id_c = *store_c.id.as_bytes();
        let addr_c = store_fee_address(&dir_key, &id_c, NetworkMode::Testnet4.bitcoin()).unwrap();
        let tx_c = pay_fee_tx(&chain, addr_c.script_pubkey(), FEE_SATS, 4);
        chain.mine();
        let admit_c = Admission::sign(id_c, tx_c.compute_txid().to_byte_array(), 0, chain.tip(), &seller_c_secret).unwrap();
        intro.seed_replica.lock().unwrap().accept_admission(&admit_c.encode().unwrap()).unwrap();
        let (seq2, bytes2) = intro
            .try_issue_admission_index_with(&dir_secret, &dir_key)
            .unwrap()
            .expect("new seller bumps OBAX to seq 2");
        assert_eq!(seq2, 2);
        let snap2 = AdmissionIndex::decode(&bytes2).unwrap();
        let mut expected2 = vec![id_a, id_b, id_c];
        expected2.sort();
        assert_eq!(snap2.stores, expected2);

        // 7. Advance past EPOCH_BLOCKS -> expired sellers are dropped in seq 3.
        *chain.0.tip.lock().unwrap() = chain.tip() + EPOCH_BLOCKS + 10;
        let (seq3, bytes3) = intro
            .try_issue_admission_index_with(&dir_secret, &dir_key)
            .unwrap()
            .expect("expired admissions update OBAX to seq 3");
        assert_eq!(seq3, 3);
        let snap3 = AdmissionIndex::decode(&bytes3).unwrap();
        assert!(snap3.stores.is_empty(), "all expired stores removed from OBAX");
    }

    #[test]
    fn transient_chain_error_preserves_existing_obax() {
        use crate::harness::TempDir;
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let node = Node::open_local(
            &dir.0,
            crate::load_catalog(NetworkMode::Testnet4).unwrap(),
            crate::NodeConfig::offline(NetworkMode::Testnet4),
            crate::LocalConfig::headless(),
        )
        .unwrap();
        let dir_secret = secret_from_label("offline-chain-dir");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(1, vec![onion('a')], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let (seller_secret, store) = shop("offline-seller", "Oak Works");
        let id = *store.id.as_bytes();
        let admit = Admission::sign(id, [5u8; 32], 0, 100, &seller_secret).unwrap();
        let existing = AdmissionIndex::sign(2, 20261123, 1, vec![id], &dir_secret).unwrap();
        {
            let mut replica = node.seed_replica.lock().unwrap();
            replica.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
            replica.accept_admission(&admit.encode().unwrap()).unwrap();
            replica.accept_admission_index(&existing.encode().unwrap(), &dir_key).unwrap();
        }
        assert!(
            node.try_issue_admission_index_with(&dir_secret, &dir_key).is_err(),
            "offline chain must return Err rather than wiping OBAX"
        );
        let replica = node.seed_replica.lock().unwrap();
        assert_eq!(replica.admission_index_sequence(), 2);
        assert_eq!(replica.current_snapshot().unwrap().stores, vec![id]);
    }

    #[test]
    fn older_admission_cannot_downgrade_newer_admission() {
        let dir = TempDir::new();
        let mut replica = Replica::open(&dir.0).unwrap();
        let secret = secret_from_label("downgrade-seller");
        let id = xonly(&secret);
        let newer = Admission::sign(id, [2u8; 32], 0, 200, &secret).unwrap();
        let older = Admission::sign(id, [1u8; 32], 0, 100, &secret).unwrap();
        replica.accept_admission(&newer.encode().unwrap()).unwrap();
        replica.accept_admission(&older.encode().unwrap()).unwrap();
        assert_eq!(replica.admissions.get(&id).unwrap().height, 200);
        replica.keep_admission_page(&older.encode().unwrap()).unwrap();
        assert_eq!(replica.admissions.get(&id).unwrap().height, 200);
    }

    #[test]
    fn pass_introductions_filters_out_own_onion() {
        use crate::harness::{self, MockChain, TempDir};
        use crate::network::NetworkMode;

        let dir = TempDir::new();
        let chain = MockChain::new();
        let intro_a = harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        let onion_a = tormail::onion_for_seed(&[1u8; 32]).to_string();
        let onion_b = tormail::onion_for_seed(&[2u8; 32]).to_string();
        let dir_secret = secret_from_label("self-filter-dir");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(1, vec![onion_a.clone(), onion_b.clone()], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        intro_a.seed_replica.lock().unwrap().accept_document(&head.encode().unwrap(), &dir_key).unwrap();
        intro_a.publish_onion(&tormail::OnionAddress::parse(&onion_a).unwrap()).unwrap();
        let peers: Vec<String> = intro_a.pass_introductions(0).into_iter().map(|i| i.onion).collect();
        assert_eq!(peers, vec![onion_b]);
    }

    #[test]
    fn obax_propagates_from_intro_a_to_intro_b_and_moderator_via_socks5() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use bitcoin::hashes::Hash;
        use openbazaar_protocol::FEE_SATS;
        use crate::admission::store_fee_address;
        use crate::harness::{self, MockChain, TempDir};
        use crate::network::NetworkMode;

        let dir_secret = secret_from_label("socks5-prop-dir");
        let dir_key = xonly(&dir_secret);
        let onion_a = tormail::onion_for_seed(&[11u8; 32]).to_string();
        let onion_b = tormail::onion_for_seed(&[22u8; 32]).to_string();
        let head = SeedDocument::sign(
            1,
            vec![onion_a, onion_b.clone()],
            DirectoryTerms::standard(dir_key),
            None,
            &dir_secret,
        )
        .unwrap();

        let chain = MockChain::new();
        let intro_a_dir = TempDir::new();
        let intro_b_dir = TempDir::new();
        let mod_dir = TempDir::new();

        let intro_a = harness::open_node(&intro_a_dir.0, NetworkMode::Testnet4, &chain, 1, harness::unreachable_escrow());
        intro_a.seed_replica.lock().unwrap().accept_document(&head.encode().unwrap(), &dir_key).unwrap();

        let intro_b_replica = Arc::new(Mutex::new(Replica::open(&intro_b_dir.0).unwrap()));
        intro_b_replica.lock().unwrap().accept_document(&head.encode().unwrap(), &dir_key).unwrap();

        // Start a local mock SOCKS5 proxy that speaks SOCKS5 + OBQR/OBSB directly to `intro_b_replica`.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_url = format!("socks5h://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = Arc::clone(&stop);
        let rep_srv = Arc::clone(&intro_b_replica);
        let server = thread::spawn(move || {
            listener.set_nonblocking(true).ok();
            while !stop_srv.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                // SOCKS5 greeting
                let mut g = [0u8; 2];
                if stream.read_exact(&mut g).is_err() { continue; }
                let mut methods = vec![0u8; g[1] as usize];
                if stream.read_exact(&mut methods).is_err() { continue; }
                // Choose username/password auth (method 2) to exercise IsolateSOCKSAuth path.
                let _ = stream.write_all(&[5, 2]);
                let mut u_hdr = [0u8; 2];
                if stream.read_exact(&mut u_hdr).is_err() { continue; }
                let mut user = vec![0u8; u_hdr[1] as usize];
                if stream.read_exact(&mut user).is_err() { continue; }
                let mut p_len = [0u8; 1];
                if stream.read_exact(&mut p_len).is_err() { continue; }
                let mut pass = vec![0u8; p_len[0] as usize];
                if stream.read_exact(&mut pass).is_err() { continue; }
                let _ = stream.write_all(&[1, 0]);
                // SOCKS5 CONNECT
                let mut req = [0u8; 5];
                if stream.read_exact(&mut req).is_err() { continue; }
                let mut host = vec![0u8; req[4] as usize];
                if stream.read_exact(&mut host).is_err() { continue; }
                let mut port = [0u8; 2];
                if stream.read_exact(&mut port).is_err() { continue; }
                let _ = stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]);

                // Directory frame: OBQR or OBSB
                let mut magic = [0u8; 4];
                if stream.read_exact(&mut magic).is_err() { continue; }
                let mut vh = [0u8; 2];
                if stream.read_exact(&mut vh).is_err() { continue; }
                let kind = vh[1];
                if &magic == QUERY_MAGIC {
                    let mut id = [0u8; 32];
                    if stream.read_exact(&mut id).is_err() { continue; }
                    let reply = rep_srv.lock().unwrap().answer(kind, &id);
                    match reply {
                        Some(body) => {
                            let _ = stream.write_all(&[0]);
                            let _ = stream.write_all(&(body.len() as u32).to_le_bytes());
                            let _ = stream.write_all(&body);
                        }
                        None => {
                            let _ = stream.write_all(&[1, 0, 0, 0, 0]);
                        }
                    }
                } else if &magic == SUBMIT_MAGIC {
                    let mut lb = [0u8; 4];
                    if stream.read_exact(&mut lb).is_err() { continue; }
                    let len = u32::from_le_bytes(lb) as usize;
                    let mut body = vec![0u8; len];
                    if len > 0 && stream.read_exact(&mut body).is_err() { continue; }
                    let mut rep = rep_srv.lock().unwrap();
                    let ok = match kind {
                        tormail::SUBMIT_STORE => rep.accept_store(&body).is_ok(),
                        tormail::SUBMIT_ADMISSION => rep.accept_admission(&body).is_ok(),
                        tormail::SUBMIT_ADMISSION_INDEX => rep.accept_admission_index(&body, &dir_key).is_ok(),
                        _ => false,
                    };
                    let _ = stream.write_all(&[if ok { 0 } else { 1 }]);
                }
            }
        });

        // Seller pays fee, signs admission, submits store + admission to intro_a and intro_b via SOCKS5.
        let (seller_secret, store) = shop("socks5-seller", "Harbor Craft");
        let id = *store.id.as_bytes();
        let addr = store_fee_address(&dir_key, &id, NetworkMode::Testnet4.bitcoin()).unwrap();
        let tx = pay_fee_tx(&chain, addr.script_pubkey(), FEE_SATS, 10);
        chain.mine();
        let admit = Admission::sign(id, tx.compute_txid().to_byte_array(), 0, chain.tip(), &seller_secret).unwrap();

        // Simulate vendor submitting store + admission through SOCKS5 proxy (no Arti tor-client created).
        let vendor_state = TempDir::new();
        assert!(submit_record(&vendor_state.0, &onion_b, Some(&proxy_url), tormail::SUBMIT_STORE, &store.encode().unwrap()).unwrap());
        assert!(submit_record(&vendor_state.0, &onion_b, Some(&proxy_url), tormail::SUBMIT_ADMISSION, &admit.encode().unwrap()).unwrap());
        assert!(!vendor_state.0.join("state").exists(), "SOCKS5 path must not open Arti");

        // intro_a receives admission, auto-issues OBAX seq 1, and propagates to intro_b via SOCKS5.
        intro_a.seed_replica.lock().unwrap().accept_admission(&admit.encode().unwrap()).unwrap();
        let (seq, obax_bytes) = intro_a
            .try_issue_admission_index_with(&dir_secret, &dir_key)
            .unwrap()
            .expect("intro_a issues OBAX seq 1");
        assert_eq!(seq, 1);
        assert!(submit_record(&intro_a_dir.0, &onion_b, Some(&proxy_url), tormail::SUBMIT_ADMISSION_INDEX, &obax_bytes).unwrap());

        // Moderator pulls from intro_b via SOCKS5 and sees the seller admitted!
        let source_b = Introduction {
            onion: onion_b,
            state_dir: mod_dir.0.join("tor-client"),
            proxy: Some(proxy_url),
        };
        let mut moderator = Replica::open(&mod_dir.0).unwrap();
        moderator.pull(&[&source_b], &dir_key, [0u8; 32]).unwrap();
        assert_eq!(moderator.admission_index_sequence(), 1);
        assert!(moderator.snapshot_names(&id), "moderator received OBAX naming the seller");
        assert!(moderator.stores.contains_key(&id), "moderator fetched the seller's store record");

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
