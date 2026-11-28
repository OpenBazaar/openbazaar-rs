//! Stores this node follows on other nodes, kept as signed copies.
//!
//! A follow is the base URL of another node: loopback HTTP, or `http://` and
//! a v3 onion reached through this node's Tor client. Every round the node
//! reads `GET {url}/public/store`, the
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

use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::now_secs;
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "replicas.obv";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FOLLOWS: usize = 64;
const MAX_ADMITTED_REPLICAS: usize = 512;
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
    pub admitted: Vec<Follow>,
}

/// A base URL as followed: loopback `http://` or a v3 onion, no path.
pub(crate) fn normalize(url: &str) -> Result<String, String> {
    crate::peer_http::sale_base(url)
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

fn decode_follows(items: Option<&Vec<Value>>) -> Vec<Follow> {
    let mut follows = Vec::new();
    for item in items.into_iter().flatten() {
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
    follows
}

fn encode_follows(follows: &[Follow]) -> Result<Vec<Value>, String> {
    let mut items = Vec::with_capacity(follows.len());
    for follow in follows {
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
    Ok(items)
}

impl Replicas {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let (follows, admitted) = if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            (
                decode_follows(value["follows"].as_array()),
                decode_follows(value["admitted"].as_array()),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(Self { path, vault, follows, admitted })
    }

    fn save(&self) -> Result<(), String> {
        let follows = encode_follows(&self.follows)?;
        let admitted = encode_follows(&self.admitted)?;
        let bytes = serde_json::to_vec(&json!({ "v": 1, "follows": follows, "admitted": admitted })).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())
    }

    /// The copies this node serves because it explicitly follows their URL.
    pub(crate) fn served_copies(&self) -> Vec<Replica> {
        self.follows.iter().filter_map(|follow| follow.replica.clone()).collect()
    }

    /// The copies this node serves because they are directory-admitted stores.
    pub(crate) fn served_admitted_copies(&self) -> Vec<Replica> {
        self.admitted.iter().filter_map(|follow| follow.replica.clone()).collect()
    }

    /// The followed or admitted URL serving store `id`, if any.
    pub(crate) fn url_of(&self, id: &StoreId) -> Option<&str> {
        self.follows
            .iter()
            .chain(self.admitted.iter())
            .find(|follow| follow.replica.as_ref().is_some_and(|replica| replica.store.id == *id))
            .map(|follow| follow.url.as_str())
    }
}

/// Put `replica` in `catalog`, replacing any older copy of that store.
pub(crate) fn serve_replica(catalog: &mut Catalog, replica: &Replica) {
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
        let refused = |replica: &Replica| {
            replica.store.id == *own
                || crate::demo_secret(&replica.store).is_some()
                || demo_sellers.contains(&replica.store.name)
        };
        for follow in &replicas.admitted {
            let Some(replica) = &follow.replica else { continue };
            if refused(replica) || !catalog.stores.iter().any(|store| store.id == replica.store.id) {
                continue;
            }
            serve_replica(catalog, replica);
        }
        for follow in &replicas.follows {
            let Some(replica) = &follow.replica else { continue };
            if refused(replica) {
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
        if self.own_url() == Some(url.as_str()) || self.sale_reply_url().as_deref() == Some(url.as_str()) {
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
        json!({
            "follows": follows,
            "seeds": [{ "url": crate::test_network::test_network_seed() }],
        })
    }

    /// Keep `replicas.admitted` aligned with the directory's currently shown
    /// stores so every admitted vendor's `/public/store` and `/public/media`
    /// replicate automatically over Tor.
    fn sync_admitted_follows(&self) {
        let wanted = self.admitted_replica_urls();
        let mut replicas = self.replicas();
        let mut changed = false;
        let before = replicas.admitted.len();
        let followed_urls: Vec<String> = replicas.follows.iter().map(|follow| follow.url.clone()).collect();
        replicas
            .admitted
            .retain(|entry| wanted.contains(&entry.url) && !followed_urls.contains(&entry.url));
        if replicas.admitted.len() != before {
            changed = true;
        }
        for url in wanted {
            if followed_urls.contains(&url) || replicas.admitted.iter().any(|entry| entry.url == url) {
                continue;
            }
            if replicas.admitted.len() >= MAX_ADMITTED_REPLICAS {
                break;
            }
            replicas.admitted.push(Follow { url, replica: None, fetched: 0, error: None });
            changed = true;
        }
        if changed {
            if let Err(err) = replicas.save() {
                eprintln!("ob-node: {FILE}: {err}");
            }
        }
    }

    /// Fetch every followed and directory-admitted store once. Returns how many copies changed.
    pub(crate) fn replicate_once(&self) -> usize {
        self.sync_admitted_follows();
        let urls: Vec<String> = {
            let replicas = self.replicas();
            replicas
                .follows
                .iter()
                .chain(replicas.admitted.iter())
                .map(|follow| follow.url.clone())
                .collect()
        };
        urls.iter().filter(|url| self.replicate(url) | (self.pull_reviews(url) > 0)).count()
    }

    /// Fetch the store at `url`, keep it, and serve it. `true` when the copy changed.
    fn replicate(&self, url: &str) -> bool {
        let fetched = fetch_store(&self.peers, url).and_then(|(replica, moderators)| match self.refuse_replica(&replica.store) {
            Some(reason) => Err(reason),
            None => {
                self.learn_moderators(&moderators);
                Ok(replica)
            }
        });
        if let Ok(replica) = &fetched {
            self.fetch_images(url, replica);
        }
        let mut replicas = self.replicas();
        let (changed, stale) = {
            let inner = &mut *replicas;
            let Some(follow) = inner
                .follows
                .iter_mut()
                .chain(inner.admitted.iter_mut())
                .find(|follow| follow.url == url)
            else {
                return false;
            };
            match fetched {
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
                    if changed {
                        eprintln!("ob-node: replica {url}: {err}");
                    }
                    follow.error = Some(err);
                    (changed, false)
                }
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
            match fetch_image(&self.peers, url, &hash) {
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

/// The store at `url`, and the signed profiles of the moderators it accepts
/// as its node relays them. The profiles are checked where they are kept
/// ([`Node::learn_moderators`]).
fn fetch_store(peers: &crate::peer_http::PeerLink, url: &str) -> Result<(Replica, Vec<Value>), String> {
    let answer = crate::peer_http::call(peers, "GET", &format!("{url}/public/store"), None, None, FETCH_TIMEOUT, MAX_STORE_BODY as usize)?;
    if answer.status == 404 {
        return Err("that node is not selling".into());
    }
    if !(200..300).contains(&answer.status) {
        return Err(format!("{url}: {}", answer.status));
    }
    if answer.body.len() as u64 > MAX_STORE_BODY {
        return Err("the store's answer is too large".into());
    }
    let value: Value = serde_json::from_slice(&answer.body).map_err(|_| "the store's answer could not be read".to_string())?;
    let store = value["store"].as_str().ok_or("the store's answer has no store record")?;
    let moderators = value["moderators"].as_array().cloned().unwrap_or_default();
    decode_records(store, value["listings"].as_array().map_or(&[][..], Vec::as_slice)).map(|replica| (replica, moderators))
}

fn fetch_image(peers: &crate::peer_http::PeerLink, url: &str, hash: &[u8; 32]) -> Result<Vec<u8>, String> {
    let answer = crate::peer_http::call(
        peers,
        "GET",
        &format!("{url}/public/media/{}", hex(hash)),
        None,
        None,
        FETCH_TIMEOUT,
        crate::media::MAX_MEDIA,
    )?;
    if !(200..300).contains(&answer.status) {
        return Err(format!("{url}: {}", answer.status));
    }
    if answer.body.len() > crate::media::MAX_MEDIA {
        return Err("the image is too large".into());
    }
    Ok(answer.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_fetch_does_not_follow_a_redirect_to_another_host() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        let trap = TcpListener::bind("127.0.0.1:0").unwrap();
        let trap_port = trap.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        thread::spawn(move || {
            for mut stream in trap.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
            }
        });
        let seller = TcpListener::bind("127.0.0.1:0").unwrap();
        let seller_port = seller.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut stream in seller.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{trap_port}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(body.as_bytes());
            }
        });

        let err = match fetch_store(&crate::peer_http::PeerLink::default(), &format!("http://127.0.0.1:{seller_port}")) {
            Err(err) => err,
            Ok(_) => panic!("the redirect was treated as a store"),
        };
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the buyer's catalog fetch followed the redirect: {err}");
        assert!(err.contains("could not be read") || err.contains("302") || err.contains(seller_port.to_string().as_str()), "{err}");
    }

    #[test]
    fn clearnet_base_urls_are_not_followed() {
        let onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";
        assert_eq!(normalize(" http://127.0.0.1:9000/ ").unwrap(), "http://127.0.0.1:9000");
        assert_eq!(normalize(&format!("http://{onion}/")).unwrap(), format!("http://{onion}"));
        assert!(normalize("http://10.0.0.1:9000").is_err());
        assert!(normalize("https://127.0.0.1:9000").is_err());
        assert!(normalize("http://127.0.0.1:9000/public").is_err());
        assert!(normalize(&format!("https://{onion}")).is_err());
        assert!(normalize(&format!("http://{onion}/public")).is_err());
    }

    #[test]
    fn admitted_vendor_store_listings_and_media_replicate_to_moderator_catalog_and_media() {
        use std::sync::Arc;

        use openbazaar_protocol::{secret_from_label, xonly, AdmissionIndex, DirectoryTerms, SeedDocument};

        use crate::harness::{self, MockChain, TempDir};
        use crate::http::dispatch;
        use crate::network::NetworkMode;
        use crate::peer_http::OnionDial;
        use crate::seller_store_tests::{mug, peer_request, png, upload};

        let vendor_onion = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";
        let chain = MockChain::new();
        let vendor_dir = TempDir::new();
        let vendor = Arc::new(harness::open_node(
            &vendor_dir.0,
            NetworkMode::Testnet4,
            &chain,
            1,
            harness::unreachable_escrow(),
        ));
        let (status, _) = harness::request(&vendor, "PUT", "/store/role", r#"{"role":"seller"}"#);
        assert!(status.contains("200"), "{status}");
        let avatar_bytes = png(11);
        let (_, avatar) = upload(&vendor, &avatar_bytes);
        let avatar_hash = avatar["hash"].as_str().unwrap().to_string();
        let profile = json!({
            "name": "Kiln & Clay",
            "tagline": "Stoneware studio",
            "about": "Small-batch ceramics.",
            "avatar": avatar_hash,
            "shipsTo": ["US"],
            "shippingPolicy": "Ships in 3 days.",
        });
        let (status, saved) = harness::request(&vendor, "PUT", "/store/profile", &profile.to_string());
        assert!(status.contains("200"), "{status} {saved}");
        let photo_bytes = png(22);
        let (_, photo) = upload(&vendor, &photo_bytes);
        let photo_hash = photo["hash"].as_str().unwrap().to_string();
        let (status, created) = harness::request(&vendor, "POST", "/store/listings", &mug(&photo_hash).to_string());
        assert!(status.contains("200"), "{status} {created}");
        let base_store = vendor.own_record().expect("vendor has a signed store record");
        let mut details = base_store.details.clone().unwrap();
        details.onion = vendor_onion.to_string();
        let vendor_secret = vendor.own_signing_key().expect("vendor signing key");
        let vendor_store = Store::sign(
            base_store.name.clone(),
            base_store.region.clone(),
            base_store.direct_pay.clone(),
            base_store.rating.unwrap(),
            details,
            &vendor_secret,
        )
        .unwrap();
        vendor.serve_own(vendor_store.clone());

        struct FakeVendorOnion {
            expected_onion: String,
            vendor: Arc<Node>,
        }
        impl OnionDial for FakeVendorOnion {
            fn exchange(&self, onion: &str, request: &[u8], _max: usize) -> Result<Vec<u8>, String> {
                if onion != self.expected_onion {
                    return Err(format!("unexpected onion {onion}"));
                }
                // Simulate sale_gate rewriting the .onion Host header to loopback before forwarding.
                let text = String::from_utf8_lossy(request).replace(&format!("Host: {onion}"), "Host: 127.0.0.1");
                Ok(dispatch(&self.vendor, text.as_bytes()))
            }
        }

        let mod_dir = TempDir::new();
        let moderator = harness::open_node(
            &mod_dir.0,
            NetworkMode::Testnet4,
            &chain,
            1,
            harness::unreachable_escrow(),
        );
        moderator.peers.use_fake(Arc::new(FakeVendorOnion {
            expected_onion: vendor_onion.to_string(),
            vendor: Arc::clone(&vendor),
        }));

        // Admit the vendor via OBAX snapshot + Store record (no explicit /follows, no listings in directory).
        let dir_secret = secret_from_label("admitted-replication-dir");
        let dir_key = xonly(&dir_secret);
        let head = SeedDocument::sign(2, vec![vendor_onion.to_string()], DirectoryTerms::standard(dir_key), None, &dir_secret).unwrap();
        let snapshot = AdmissionIndex::sign(1, 20261128, 2, vec![*vendor_store.id.as_bytes()], &dir_secret).unwrap();
        {
            let mut replica = moderator.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_document(&head.encode().unwrap(), &dir_key).unwrap();
            replica.accept_store(&vendor_store.encode().unwrap()).unwrap();
            replica.accept_admission_index(&snapshot.encode().unwrap(), &dir_key).unwrap();
        }
        moderator.refresh_market();

        let catalog_listings_for_vendor = |node: &Node| -> Vec<Listing> {
            let (status, body) = peer_request(node, "/public/catalog");
            assert!(status.contains("200"), "{status}");
            let value: Value = serde_json::from_slice(&body).unwrap();
            for entry in value["stores"].as_array().unwrap() {
                let bytes = unhex(entry["store"].as_str().unwrap()).unwrap();
                let store = Store::decode(&bytes).unwrap();
                if store.id == vendor_store.id {
                    return entry["listings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|hex_str| {
                            let raw = unhex(hex_str.as_str().unwrap()).unwrap();
                            let listing = Listing::decode(&raw).unwrap();
                            listing.verify().unwrap();
                            listing
                        })
                        .collect();
                }
            }
            Vec::new()
        };

        // Before replication, moderator has the OBAX store with 0 listings and no vendor media.
        assert_eq!(catalog_listings_for_vendor(&moderator).len(), 0);
        let (status, _) = peer_request(&moderator, &format!("/public/media/{photo_hash}"));
        assert!(status.contains("404"), "photo should not be on moderator yet: {status}");

        // Automatic replication pulls /public/store and /public/media/<hash> over Tor.
        assert_eq!(moderator.replicate_once(), 1);
        let replicated = catalog_listings_for_vendor(&moderator);
        assert_eq!(replicated.len(), 1, "admitted vendor listing should appear in /public/catalog");
        assert_eq!(replicated[0].title, "Hand-thrown mug");

        // Listing photo and store avatar are now served same-origin from moderator /public/media/<hash>.
        let (status, served_photo) = peer_request(&moderator, &format!("/public/media/{photo_hash}"));
        assert!(status.contains("200"), "replicated photo should serve 200: {status}");
        assert_eq!(served_photo, photo_bytes);
        let (status, served_avatar) = peer_request(&moderator, &format!("/public/media/{avatar_hash}"));
        assert!(status.contains("200"), "replicated avatar should serve 200: {status}");
        assert_eq!(served_avatar, avatar_bytes);

        // Auto-replicated admitted stores do not pollute explicit GET /follows.
        assert_eq!(moderator.follows_value()["follows"].as_array().unwrap().len(), 0);

        // Subsequent directory refresh_market() must NOT wipe out the replicated listings.
        moderator.refresh_market();
        let after_refresh = catalog_listings_for_vendor(&moderator);
        assert_eq!(after_refresh.len(), 1, "refresh_market must preserve replicated admitted listings");
        assert_eq!(after_refresh[0].title, "Hand-thrown mug");

        // Persisted admitted replicas survive reload and serve at boot when the store is in catalog.
        let reloaded = Replicas::open(&moderator.data_dir, moderator.replicas().vault.clone()).unwrap();
        assert_eq!(reloaded.admitted.len(), 1);
        let mut boot_catalog = Catalog {
            stores: vec![vendor_store.clone()],
            moderator: moderator.catalog().moderator.clone(),
            listings: Vec::new(),
        };
        Node::serve_replicas_at_boot(&reloaded, &mut boot_catalog, &moderator.own_store, &[]);
        assert_eq!(boot_catalog.listings.len(), 1);
        assert_eq!(boot_catalog.listings[0].title, "Hand-thrown mug");

        // Removing the store from OBAX drops it from /public/catalog and prunes replicas.admitted.
        let empty_snapshot = AdmissionIndex::sign(2, 20261128, 2, vec![], &dir_secret).unwrap();
        {
            let mut replica = moderator.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.accept_admission_index(&empty_snapshot.encode().unwrap(), &dir_key).unwrap();
        }
        moderator.refresh_market();
        assert_eq!(catalog_listings_for_vendor(&moderator).len(), 0);
        moderator.replicate_once();
        assert!(moderator.replicas().admitted.is_empty());
    }
}
