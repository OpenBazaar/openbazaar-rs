//! The vendor's own content labels, set from the paired client.
//!
//! The node holds the key of each store it runs, so it signs the store or one
//! of its listings again with that store's key. A listing's body changes, so its content hash
//! changes and the catalog serves the new address. Choices are kept in
//! `content-labels.json` and applied again on the next boot, keyed by the
//! hash each listing had when the node loaded it. The file holds only labels
//! the catalog already publishes, so it is not sealed.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{Catalog, ContentLabel, ContentRating, ContentTags, StoreId};
use serde_json::{json, Map, Value};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::{catalog_value, Error, Node};

const FILE: &str = "content-labels.json";

pub(crate) struct VendorLabels {
    path: PathBuf,
    /// Hash of each catalog listing at boot, in catalog order.
    base: Vec<String>,
    /// Store ratings by store key hex.
    stores: BTreeMap<String, ContentRating>,
    listings: BTreeMap<String, ContentLabel>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum LabelError {
    Invalid(&'static str),
    UnknownListing,
    NotOurs,
    Failed(String),
}

impl VendorLabels {
    /// Read saved labels and sign them into `catalog`.
    pub(crate) fn open(dir: &Path, catalog: &mut Catalog, secrets: &[(StoreId, SecretKey)]) -> Result<Self, Error> {
        let mut base = Vec::with_capacity(catalog.listings.len());
        for listing in &catalog.listings {
            base.push(listing.content_hash().map_err(Error::Protocol)?.to_hex());
        }
        let mut labels = Self {
            path: dir.join(FILE),
            base,
            stores: BTreeMap::new(),
            listings: BTreeMap::new(),
        };
        let text = match fs::read_to_string(&labels.path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(labels),
            Err(err) => return Err(Error::Detail(err.to_string())),
        };
        let saved: Value = serde_json::from_str(&text).map_err(|err| Error::Detail(format!("{FILE}: {err}")))?;
        let detail = |err: LabelError| Error::Detail(format!("{FILE}: {err:?}"));
        let mut ratings: Vec<(Option<String>, &str)> = Vec::new();
        if let Some(rating) = saved["store"].as_str() {
            ratings.push((None, rating));
        }
        for (key, rating) in saved["stores"].as_object().into_iter().flatten() {
            ratings.push((Some(key.clone()), rating.as_str().unwrap_or("")));
        }
        for (key, rating) in ratings {
            let rating = ContentRating::from_name(rating).ok_or(LabelError::Invalid("rating")).map_err(detail)?;
            let Some(index) = store_index(catalog, key.as_deref()) else {
                continue;
            };
            // A locked node does not hold its own store key; the cached
            // record already carries the rating.
            match labels.apply_store(catalog, index, rating, secrets) {
                Err(LabelError::NotOurs) => {
                    labels.stores.insert(catalog.stores[index].id.to_hex(), rating);
                }
                other => other.map_err(detail)?,
            }
        }
        if let Some(listings) = saved["listings"].as_object() {
            for (base_hash, label) in listings {
                let Some(index) = labels.base.iter().position(|hash| hash == base_hash) else {
                    continue;
                };
                let label = parse_label(label).map_err(detail)?;
                labels.apply_listing(catalog, index, label, secrets).map_err(detail)?;
            }
        }
        Ok(labels)
    }

    /// Label a store (`target` is `"store"`, with `storeKey` or else the
    /// first store) or one listing by its current hash.
    ///
    /// Returns the listing's new hash, or `None` for the store.
    pub(crate) fn set(
        &mut self,
        catalog: &mut Catalog,
        request: &Value,
        secrets: &[(StoreId, SecretKey)],
    ) -> Result<Option<String>, LabelError> {
        let target = request["target"].as_str().ok_or(LabelError::Invalid("Name a target: \"store\" or a listing hash."))?;
        let new_hash = if target == "store" {
            if request.get("tags").is_some_and(|tags| tags.as_array().is_some_and(|list| !list.is_empty())) {
                return Err(LabelError::Invalid("A store carries a rating, not tags."));
            }
            let rating = parse_rating(&request["rating"])?;
            let index = store_index(catalog, request["storeKey"].as_str()).ok_or(LabelError::UnknownListing)?;
            self.apply_store(catalog, index, rating, secrets)?;
            None
        } else {
            let index = catalog
                .listings
                .iter()
                .position(|listing| listing.content_hash().is_ok_and(|hash| hash.to_hex() == target))
                .ok_or(LabelError::UnknownListing)?;
            Some(self.apply_listing(catalog, index, parse_label(request)?, secrets)?)
        };
        self.save()?;
        Ok(new_hash)
    }

    fn apply_store(
        &mut self,
        catalog: &mut Catalog,
        index: usize,
        rating: ContentRating,
        secrets: &[(StoreId, SecretKey)],
    ) -> Result<(), LabelError> {
        let store = &catalog.stores[index];
        let store = store.clone().rerate(rating, secret_of(secrets, &store.id)?).map_err(not_ours)?;
        self.stores.insert(store.id.to_hex(), rating);
        catalog.stores[index] = store;
        Ok(())
    }

    fn apply_listing(
        &mut self,
        catalog: &mut Catalog,
        index: usize,
        label: ContentLabel,
        secrets: &[(StoreId, SecretKey)],
    ) -> Result<String, LabelError> {
        let secret = secret_of(secrets, &catalog.listings[index].store_id)?;
        let listing = catalog.listings[index].relabel(label, secret).map_err(not_ours)?;
        let hash = listing.content_hash().map_err(|err| LabelError::Failed(err.to_string()))?.to_hex();
        catalog.listings[index] = listing;
        self.listings.insert(self.base[index].clone(), label);
        Ok(hash)
    }

    fn save(&self) -> Result<(), LabelError> {
        let listings: Map<String, Value> = self
            .listings
            .iter()
            .map(|(hash, label)| (hash.clone(), json!({ "rating": label.rating.as_str(), "tags": label.tags.names() })))
            .collect();
        let stores: Map<String, Value> =
            self.stores.iter().map(|(key, rating)| (key.clone(), json!(rating.as_str()))).collect();
        let value = json!({ "stores": stores, "listings": listings });
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_vec_pretty(&value).map_err(|err| LabelError::Failed(err.to_string()))?;
        fs::write(&tmp, text)
            .and_then(|_| fs::rename(&tmp, &self.path))
            .map_err(|err| LabelError::Failed(err.to_string()))
    }
}

/// The store named by key hex, or the first store.
fn store_index(catalog: &Catalog, key: Option<&str>) -> Option<usize> {
    match key {
        None => (!catalog.stores.is_empty()).then_some(0),
        Some(key) => catalog.stores.iter().position(|store| store.id.to_hex() == key),
    }
}

fn secret_of<'a>(secrets: &'a [(StoreId, SecretKey)], id: &StoreId) -> Result<&'a SecretKey, LabelError> {
    secrets.iter().find(|(store, _)| store == id).map(|(_, secret)| secret).ok_or(LabelError::NotOurs)
}

fn not_ours(err: openbazaar_protocol::Error) -> LabelError {
    match err {
        openbazaar_protocol::Error::StoreMismatch => LabelError::NotOurs,
        other => LabelError::Failed(other.to_string()),
    }
}

fn parse_rating(value: &Value) -> Result<ContentRating, LabelError> {
    value
        .as_str()
        .and_then(ContentRating::from_name)
        .ok_or(LabelError::Invalid("The rating must be general, sensitive, or adult."))
}

fn parse_label(value: &Value) -> Result<ContentLabel, LabelError> {
    let rating = parse_rating(&value["rating"])?;
    let mut tags = ContentTags::empty();
    match &value["tags"] {
        Value::Null => {}
        Value::Array(list) => {
            for name in list {
                let tag = name
                    .as_str()
                    .and_then(ContentTags::from_name)
                    .ok_or(LabelError::Invalid("Tags must be nudity, violence, drugs, weapons, or gambling."))?;
                tags = tags.with(tag);
            }
        }
        _ => return Err(LabelError::Invalid("Tags must be a list.")),
    }
    Ok(ContentLabel::new(rating, tags))
}

/// `POST /content-labels`: paired loopback callers only.
pub(crate) fn response(node: &Node, client: &Client, head: &str, body: &[u8], origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    if body.len() > 4 * 1024 {
        return json_error("400 Bad Request", "That request is too large.", origin);
    }
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return json_error("400 Bad Request", "The label could not be read.", origin);
    };
    let result = {
        let mut labels = node.vendor_labels.lock().unwrap_or_else(|err| err.into_inner());
        let secrets = node.held_store_secrets();
        let mut catalog = node.catalog_mut();
        labels
            .set(&mut catalog, &request, &secrets)
            .and_then(|hash| Ok((hash, catalog_value(&catalog).map_err(|err| LabelError::Failed(err.to_string()))?)))
    };
    let own = node.own_store.to_hex();
    match result {
        Ok((hash, mut catalog)) => {
            if let Some(record) = node.own_record() {
                if let Err(err) = crate::own_store::write_cached(&node.data_dir, node.lock.vault(), &record) {
                    eprintln!("ob-node: own store: {err}");
                }
            }
            crate::xmr::add_xmr_flags(node, &mut catalog);
            json_body("200 OK", &json!({ "hash": hash, "catalog": catalog }), origin)
        }
        Err(LabelError::Invalid(message)) => json_error("400 Bad Request", message, origin),
        Err(LabelError::UnknownListing) => json_error("404 Not Found", "That listing is not in this shop.", origin),
        Err(LabelError::NotOurs) if request["storeKey"].as_str() == Some(own.as_str()) => {
            crate::lock_http::locked_response(node.lock.state().as_str(), origin)
        }
        Err(LabelError::NotOurs) => json_error("403 Forbidden", "This node does not hold that store's key.", origin),
        Err(LabelError::Failed(detail)) => {
            eprintln!("ob-node: {detail}");
            json_error("500 Internal Server Error", "The label could not be saved.", origin)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixture_keys, fixture_store_keys, load_catalog, NetworkMode};

    fn demo_secrets(catalog: &Catalog) -> Vec<(StoreId, SecretKey)> {
        catalog.stores.iter().map(|store| (store.id, fixture_store_keys(&store.name).unwrap().store)).collect()
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("ob-vendor-labels-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    #[test]
    fn relabel_resigns_moves_the_hash_and_survives_a_restart() {
        let dir = temp_dir("restart");
        let mut catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let secret = demo_secrets(&catalog);
        let mut labels = VendorLabels::open(&dir.0, &mut catalog, &secret).unwrap();
        let old = catalog.listings[0].content_hash().unwrap().to_hex();

        let new = labels
            .set(&mut catalog, &json!({ "target": old, "rating": "sensitive", "tags": ["weapons"] }), &secret)
            .unwrap()
            .unwrap();
        assert_ne!(new, old);
        catalog.verify().unwrap();
        let label = catalog.listings[0].content.unwrap();
        assert_eq!((label.rating, label.tags), (ContentRating::Sensitive, ContentTags::WEAPONS));

        let again = labels
            .set(&mut catalog, &json!({ "target": new, "rating": "adult" }), &secret)
            .unwrap()
            .unwrap();
        assert_eq!(labels.set(&mut catalog, &json!({ "target": "store", "rating": "adult" }), &secret), Ok(None));
        assert_eq!(catalog.stores[0].rating, Some(ContentRating::Adult));
        let hale = catalog.stores[1].id.to_hex();
        assert_eq!(
            labels.set(&mut catalog, &json!({ "target": "store", "storeKey": hale, "rating": "sensitive" }), &secret),
            Ok(None)
        );

        let mut reloaded = load_catalog(NetworkMode::Testnet4).unwrap();
        VendorLabels::open(&dir.0, &mut reloaded, &secret).unwrap();
        reloaded.verify().unwrap();
        assert_eq!(reloaded.listings[0].content_hash().unwrap().to_hex(), again);
        assert_eq!(reloaded.stores[0].rating, Some(ContentRating::Adult));
        assert_eq!(reloaded.stores[1].rating, Some(ContentRating::Sensitive));
        assert_eq!(reloaded.listings[1], catalog.listings[1]);
    }

    fn post(token: Option<&str>, body: &str) -> Vec<u8> {
        let pair = token.map(|token| format!("X-Ob-Pair: {token}\r\n")).unwrap_or_default();
        format!(
            "POST /content-labels HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n{pair}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn body(response: &[u8]) -> (String, Value) {
        let text = String::from_utf8(response.to_vec()).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), serde_json::from_str(body).unwrap())
    }

    #[test]
    fn endpoint_needs_the_pairing_token_and_the_catalog_serves_the_new_hash() {
        let dir = temp_dir("http");
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        let old = node.catalog().listings[0].content_hash().unwrap().to_hex();
        let request = json!({ "target": old, "rating": "adult", "tags": ["nudity"] }).to_string();

        let (status, _) = body(&crate::http::dispatch(&node, &post(None, &request)));
        assert!(status.contains("403"), "{status}");
        let (status, _) = body(&crate::http::dispatch(&node, &post(Some(&"0".repeat(64)), &request)));
        assert!(status.contains("403"), "{status}");
        let stranger = Client { loopback: false, port: 8787 };
        let (status, _) = body(&crate::http::respond(&node, &post(Some(node.pairing_token()), &request), stranger));
        assert!(status.contains("403"), "{status}");
        assert_eq!(node.catalog().listings[0].content_hash().unwrap().to_hex(), old);

        let (status, value) = body(&crate::http::dispatch(&node, &post(Some(node.pairing_token()), &request)));
        assert!(status.contains("200"), "{status}");
        let new = value["hash"].as_str().unwrap().to_string();
        assert_ne!(new, old);
        assert_eq!(value["catalog"]["listings"][0]["hash"], new.as_str());
        assert_eq!(value["catalog"]["listings"][0]["contentRating"], "adult");
        assert_eq!(value["catalog"]["listings"][0]["contentTags"], json!(["nudity"]));

        let (_, catalog) = body(&crate::http::dispatch(&node, b"GET /catalog HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
        assert_eq!(catalog, value["catalog"]);
        let (status, _) = body(&crate::http::dispatch(&node, format!("GET /listings/{old} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()));
        assert!(status.contains("404"), "{status}");
    }

    #[test]
    fn refuses_bad_requests_and_stores_it_cannot_sign_for() {
        let dir = temp_dir("refuse");
        let mut catalog = load_catalog(NetworkMode::Testnet4).unwrap();
        let secret = demo_secrets(&catalog);
        let mut labels = VendorLabels::open(&dir.0, &mut catalog, &secret).unwrap();
        let hash = catalog.listings[0].content_hash().unwrap().to_hex();
        let before = catalog.clone();

        for request in [
            json!({ "rating": "adult" }),
            json!({ "target": hash, "rating": "unrated" }),
            json!({ "target": hash, "rating": "adult", "tags": ["spicy"] }),
            json!({ "target": "store", "rating": "adult", "tags": ["nudity"] }),
        ] {
            assert!(matches!(labels.set(&mut catalog, &request, &secret), Err(LabelError::Invalid(_))), "{request}");
        }
        assert_eq!(
            labels.set(&mut catalog, &json!({ "target": "0".repeat(64), "rating": "adult" }), &secret),
            Err(LabelError::UnknownListing)
        );
        let stranger = [(catalog.stores[0].id, fixture_keys().buyer)];
        assert_eq!(labels.set(&mut catalog, &json!({ "target": hash, "rating": "adult" }), &stranger), Err(LabelError::NotOurs));
        assert_eq!(labels.set(&mut catalog, &json!({ "target": "store", "rating": "adult" }), &stranger), Err(LabelError::NotOurs));
        assert_eq!(labels.set(&mut catalog, &json!({ "target": "store", "rating": "adult" }), &[]), Err(LabelError::NotOurs));
        assert_eq!(catalog, before);
        assert!(!dir.0.join(FILE).exists());
    }
}
