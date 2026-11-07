//! Stores this node follows on other nodes, kept as signed copies.
//!
//! A follow is the base URL of another node (loopback HTTP for now, like
//! escrow mail). Every round the node reads `GET {url}/public/store`, the
//! signed store record and listings that node sells, checks every
//! signature, fetches the images the records name, and serves the copies in
//! its catalog beside its own. The copies are kept in `replicas.obv`, sealed
//! with the vault key (the file names who this node follows), so a seller
//! that goes offline stays browsable and orderable: the order waits in the
//! outbox until the seller is back.
//!
//! A copy never replaces the own store or a demo store, and a store whose
//! name is a demo seller's is refused, since demo sellers are looked up by
//! name. The newest copy wins: store records carry no sequence number yet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use openbazaar_protocol::{Catalog, Listing, Store, StoreId};
use serde_json::{json, Value};

use crate::escrow_mail::{check_loopback, hex, unhex};
use crate::escrow_outbox::now_secs;
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "replicas.obv";
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_FOLLOWS: usize = 64;
const MAX_LISTINGS: usize = 500;
/// The whole `GET /public/store` answer, hex-encoded records included.
const MAX_STORE_BODY: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Replica {
    pub store: Store,
    pub listings: Vec<Listing>,
}

#[derive(Clone)]
pub(crate) struct Follow {
    pub url: String,
    pub replica: Option<Replica>,
    pub fetched: u64,
    pub error: Option<String>,
}

pub(crate) struct Replicas {
    path: PathBuf,
    vault: Vault,
    pub follows: Vec<Follow>,
}

/// A base URL as followed: loopback `http://`, no trailing slash.
pub(crate) fn normalize(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/').to_string();
    check_loopback(&url)?;
    if url.trim_start_matches("http://").contains('/') {
        return Err(format!("{url} is a base URL with no path"));
    }
    Ok(url)
}

fn decode_records(store: &str, listings: &[Value]) -> Result<Replica, String> {
    let bad = |what: &str| format!("the store sent a {what} that does not verify");
    let store = Store::decode(&unhex(store)?).map_err(|_| bad("store record"))?;
    store.verify().map_err(|_| bad("store record"))?;
    if listings.len() > MAX_LISTINGS {
        return Err("the store sent too many listings".into());
    }
    let mut out = Vec::with_capacity(listings.len());
    for record in listings {
        let listing = Listing::decode(&unhex(record.as_str().unwrap_or(""))?).map_err(|_| bad("listing"))?;
        listing.verify().map_err(|_| bad("listing"))?;
        if listing.store_id != store.id || listing.seller != store.name {
            return Err("the store sent a listing it did not sign".into());
        }
        out.push(listing);
    }
    Ok(Replica { store, listings: out })
}

impl Replicas {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut follows = Vec::new();
        if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            for item in value["follows"].as_array().into_iter().flatten() {
                let Ok(url) = normalize(item["url"].as_str().unwrap_or("")) else {
                    continue;
                };
                let replica = match item["store"].as_str() {
                    Some(store) => match decode_records(store, item["listings"].as_array().map_or(&[][..], Vec::as_slice)) {
                        Ok(replica) => Some(replica),
                        Err(err) => {
                            eprintln!("ob-node: {FILE}: {url}: {err}");
                            None
                        }
                    },
                    None => None,
                };
                follows.push(Follow {
                    url,
                    replica,
                    fetched: item["fetched"].as_u64().unwrap_or(0),
                    error: item["error"].as_str().map(str::to_string),
                });
            }
        }
        Ok(Self { path, vault, follows })
    }

    fn save(&self) -> Result<(), String> {
        let mut items = Vec::with_capacity(self.follows.len());
        for follow in &self.follows {
            let mut item = json!({ "url": follow.url, "fetched": follow.fetched, "error": follow.error });
            if let Some(replica) = &follow.replica {
                item["store"] = json!(hex(&replica.store.encode().map_err(|err| err.to_string())?));
                let mut listings = Vec::with_capacity(replica.listings.len());
                for listing in &replica.listings {
                    listings.push(json!(hex(&listing.encode().map_err(|err| err.to_string())?)));
                }
                item["listings"] = json!(listings);
            }
            items.push(item);
        }
        let bytes = serde_json::to_vec(&json!({ "v": 1, "follows": items })).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())
    }

    /// The followed URL serving store `id`, if any.
    pub(crate) fn url_of(&self, id: &StoreId) -> Option<&str> {
        self.follows
            .iter()
            .find(|follow| follow.replica.as_ref().is_some_and(|replica| replica.store.id == *id))
            .map(|follow| follow.url.as_str())
    }
}

/// Put `replica` in `catalog`, replacing any older copy of that store.
fn serve_replica(catalog: &mut Catalog, replica: &Replica) {
    let id = replica.store.id;
    catalog.listings.retain(|listing| listing.store_id != id);
    match catalog.stores.iter_mut().find(|store| store.id == id) {
        Some(store) => *store = replica.store.clone(),
        None => catalog.stores.push(replica.store.clone()),
    }
    catalog.listings.extend(replica.listings.iter().cloned());
}

fn unserve(catalog: &mut Catalog, id: &StoreId) {
    catalog.listings.retain(|listing| listing.store_id != *id);
    catalog.stores.retain(|store| store.id != *id);
}

impl Node {
    pub(crate) fn replicas(&self) -> std::sync::MutexGuard<'_, Replicas> {
        self.replicas.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Why this node will not serve a copy of `store`, if it will not.
    fn refuse_replica(&self, store: &Store) -> Option<String> {
        if store.id == self.own_store {
            return Some("that is this node's own store".into());
        }
        if crate::demo_secret(store).is_some() || self.store_wallets.contains_key(&store.name) {
            return Some(format!("{} is the name of a demo store", store.name));
        }
        None
    }

    /// Serve the copies kept in `replicas.obv`, at boot.
    pub(crate) fn serve_replicas_at_boot(replicas: &Replicas, catalog: &mut Catalog, own: &StoreId, demo_sellers: &[String]) {
        for follow in &replicas.follows {
            let Some(replica) = &follow.replica else { continue };
            if replica.store.id == *own
                || crate::demo_secret(&replica.store).is_some()
                || demo_sellers.contains(&replica.store.name)
            {
                continue;
            }
            serve_replica(catalog, replica);
        }
    }

    /// Follow the store at `url` and fetch it now. Answers the follow list.
    pub(crate) fn follow(&self, url: &str) -> Result<Value, String> {
        let url = self.add_follow(url)?;
        self.replicate(&url);
        Ok(self.follows_value())
    }

    /// Follow the store at `url` from the next replication round on
    /// (`ob-node serve --follow URL`). Answers the normalized URL.
    pub fn add_follow(&self, url: &str) -> Result<String, String> {
        let url = normalize(url)?;
        if self.own_url() == Some(url.as_str()) {
            return Err("That is this node's own address.".into());
        }
        let mut replicas = self.replicas();
        if !replicas.follows.iter().any(|follow| follow.url == url) {
            if replicas.follows.len() >= MAX_FOLLOWS {
                return Err("This node already follows the most stores it keeps.".into());
            }
            replicas.follows.push(Follow { url: url.clone(), replica: None, fetched: 0, error: None });
            replicas.save()?;
        }
        Ok(url)
    }

    /// Stop following `url` and stop serving its store.
    pub(crate) fn unfollow(&self, url: &str) -> Result<Value, String> {
        let url = normalize(url)?;
        let removed = {
            let mut replicas = self.replicas();
            let Some(index) = replicas.follows.iter().position(|follow| follow.url == url) else {
                return Err("This node does not follow that store.".into());
            };
            let removed = replicas.follows.remove(index);
            replicas.save()?;
            removed
        };
        if let Some(replica) = removed.replica {
            unserve(&mut self.catalog_mut(), &replica.store.id);
        }
        Ok(self.follows_value())
    }

    pub(crate) fn follows_value(&self) -> Value {
        let replicas = self.replicas();
        let follows: Vec<Value> = replicas
            .follows
            .iter()
            .map(|follow| {
                json!({
                    "url": follow.url,
                    "storeId": follow.replica.as_ref().map(|replica| replica.store.id.to_hex()),
                    "name": follow.replica.as_ref().map(|replica| replica.store.name.clone()),
                    "listings": follow.replica.as_ref().map_or(0, |replica| replica.listings.len()),
                    "fetched": follow.fetched,
                    "error": follow.error,
                })
            })
            .collect();
        json!({ "follows": follows })
    }

    /// Fetch every followed store once. Returns how many copies changed.
    pub(crate) fn replicate_once(&self) -> usize {
        let urls: Vec<String> = self.replicas().follows.iter().map(|follow| follow.url.clone()).collect();
        urls.iter().filter(|url| self.replicate(url) | (self.pull_reviews(url) > 0)).count()
    }

    /// Fetch the store at `url`, keep it, and serve it. `true` when the copy changed.
    fn replicate(&self, url: &str) -> bool {
        let fetched = fetch_store(url).and_then(|replica| match self.refuse_replica(&replica.store) {
            Some(reason) => Err(reason),
            None => Ok(replica),
        });
        if let Ok(replica) = &fetched {
            self.fetch_images(url, replica);
        }
        let mut replicas = self.replicas();
        let Some(follow) = replicas.follows.iter_mut().find(|follow| follow.url == url) else {
            return false;
        };
        let (changed, stale) = match fetched {
            Ok(replica) => {
                let old = follow.replica.as_ref();
                let changed = old.is_none_or(|old| old.store != replica.store || old.listings != replica.listings);
                let stale = old.map(|old| old.store.id).filter(|id| *id != replica.store.id);
                {
                    let mut catalog = self.catalog_mut();
                    if let Some(id) = stale {
                        unserve(&mut catalog, &id);
                    }
                    serve_replica(&mut catalog, &replica);
                }
                follow.replica = Some(replica);
                follow.fetched = now_secs();
                follow.error = None;
                (changed, stale.is_some())
            }
            Err(err) => {
                let changed = follow.error.as_deref() != Some(err.as_str());
                follow.error = Some(err);
                (changed, false)
            }
        };
        if changed || stale {
            if let Err(err) = replicas.save() {
                eprintln!("ob-node: {FILE}: {err}");
            }
        }
        changed
    }

    /// Fetch each image the store and its listings name that this node lacks.
    fn fetch_images(&self, url: &str, replica: &Replica) {
        let mut hashes: Vec<[u8; 32]> = Vec::new();
        if let Some(details) = &replica.store.details {
            hashes.extend(details.profile.avatar);
            hashes.extend(details.profile.banner);
        }
        for listing in &replica.listings {
            hashes.extend(listing.details.iter().flat_map(|details| details.images.iter().copied()));
        }
        hashes.sort();
        hashes.dedup();
        for hash in hashes {
            if self.media.get(&hash).is_some() {
                continue;
            }
            match fetch_image(url, &hash) {
                Ok(bytes) => match self.media.put(&bytes) {
                    Ok((got, _)) if got == hash => {}
                    Ok(_) => eprintln!("ob-node: {url} sent an image that does not match its hash"),
                    Err(err) => eprintln!("ob-node: image from {url}: {err}"),
                },
                Err(err) => eprintln!("ob-node: image from {url}: {err}"),
            }
        }
    }

    /// Run [`Node::replicate_once`] every `every` until `stop` is set.
    pub(crate) fn run_replicas(&self, every: Duration, stop: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        while !stop.load(Ordering::Relaxed) {
            self.replicate_once();
            let mut slept = Duration::ZERO;
            while slept < every && !stop.load(Ordering::Relaxed) {
                let step = Duration::from_millis(250).min(every - slept);
                std::thread::sleep(step);
                slept += step;
            }
        }
    }
}

fn fetch_store(url: &str) -> Result<Replica, String> {
    use std::io::Read;
    let response = ureq::get(&format!("{url}/public/store"))
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|err| match err {
            ureq::Error::Status(404, _) => "that node is not selling".to_string(),
            other => format!("{url}: {other}"),
        })?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_STORE_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|err| err.to_string())?;
    if body.len() as u64 > MAX_STORE_BODY {
        return Err("the store's answer is too large".into());
    }
    let value: Value = serde_json::from_slice(&body).map_err(|_| "the store's answer could not be read".to_string())?;
    let store = value["store"].as_str().ok_or("the store's answer has no store record")?;
    decode_records(store, value["listings"].as_array().map_or(&[][..], Vec::as_slice))
}

fn fetch_image(url: &str, hash: &[u8; 32]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let response = ureq::get(&format!("{url}/public/media/{}", hex(hash)))
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|err| err.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(crate::media::MAX_MEDIA as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_base_urls_are_followed() {
        assert_eq!(normalize(" http://127.0.0.1:9000/ ").unwrap(), "http://127.0.0.1:9000");
        assert!(normalize("http://10.0.0.1:9000").is_err());
        assert!(normalize("https://127.0.0.1:9000").is_err());
        assert!(normalize("http://127.0.0.1:9000/public").is_err());
    }
}
