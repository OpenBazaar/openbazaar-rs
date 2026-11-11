//! The store records this node serves, and `GET /stores` and `GET /stores/{key}`.
//!
//! The node's own store ([`crate::own_store`]) is signed with the store key
//! from its backup phrase (`m/1984'/coin'/0'/0'`) and publishes the mailbox
//! key from `m/1984'/coin'/3'/0'`. On testnet4 the node also serves the demo
//! stores, whose keys are public fixtures.
//!
//! Every key a letter is sealed to comes from a signed store record in the
//! catalog. Fixture secrets are used only to open letters to, and sign as,
//! the demo stores this node plays on testnet4.

use std::sync::atomic::Ordering;

use bitcoin::secp256k1::{PublicKey, SecretKey, XOnlyPublicKey};
use bitcoin::Amount;
use openbazaar_pay::Bolt12Offer;
use openbazaar_protocol::{fixture_store_keys, Catalog, DirectPay, Store, StoreId};
use serde_json::{json, Map, Value};
use tormail::{StoreKey, StoreSecret};

use crate::http::{json_body, json_error, local_guard, Client};
use crate::network::NetworkMode;
use crate::{listing_value, rating_name, Error, Node};

/// The price in a store-level BOLT 12 offer. Each order gets its own offer for its total.
pub(crate) const STORE_OFFER_SATS: u64 = 18_000;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A BOLT 12 offer for `store` and the store's BIP 352 code, for `network`.
pub(crate) fn direct_pay(store: PublicKey, silent_payment_code: String, name: &str, network: NetworkMode) -> Result<DirectPay, Error> {
    let offer = Bolt12Offer::create(store, Amount::from_sat(STORE_OFFER_SATS), name, network.bitcoin())
        .map_err(|err| Error::Detail(err.to_string()))?;
    Ok(DirectPay { bolt12_offer: offer.to_string(), silent_payment_code })
}

pub(crate) fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}


pub(crate) fn mailbox_key(store: &Store) -> Result<StoreKey, String> {
    let details = store
        .details
        .as_ref()
        .ok_or_else(|| format!("{} has no mailbox key in its signed record", store.name))?;
    StoreKey::from_bytes(details.mailbox_key).map_err(|err| err.to_string())
}

impl Node {
    /// The store record named `seller`, as signed in the catalog.
    pub(crate) fn seller_store(&self, seller: &str) -> Result<Store, String> {
        self.catalog()
            .stores
            .iter()
            .find(|store| store.name == seller)
            .cloned()
            .ok_or_else(|| format!("{seller} has no store record"))
    }

    /// The key letters to `seller` are sealed to: always from its signed record.
    pub(crate) fn seller_mail_key(&self, seller: &str) -> Result<StoreKey, String> {
        mailbox_key(&self.seller_store(seller)?)
    }

    /// The x-only store key of `seller`, from its signed record.
    pub(crate) fn seller_store_key(&self, seller: &str) -> Result<XOnlyPublicKey, String> {
        XOnlyPublicKey::from_slice(self.seller_store(seller)?.id.as_bytes()).map_err(|err| err.to_string())
    }

    /// The signing secret of a store this node holds: its own (not while
    /// locked; see [`crate::own_store`]), or on testnet4 a demo store.
    pub(crate) fn store_secret(&self, id: &StoreId) -> Option<SecretKey> {
        if *id == self.own_store {
            return self.own_signing_key();
        }
        if self.network != NetworkMode::Testnet4 {
            return None;
        }
        crate::demo_secret(self.catalog().store(id)?)
    }

    /// Every store key this node holds, for re-signing labels.
    pub(crate) fn held_store_secrets(&self) -> Vec<(StoreId, SecretKey)> {
        let ids: Vec<StoreId> = self.catalog().stores.iter().map(|store| store.id).collect();
        ids.into_iter().filter_map(|id| self.store_secret(&id).map(|secret| (id, secret))).collect()
    }

    pub(crate) fn seller_store_secret(&self, seller: &str) -> Result<SecretKey, String> {
        let id = self.seller_store(seller)?.id;
        self.store_secret(&id).ok_or_else(|| format!("this node does not hold the store key of {seller}"))
    }

    /// The mailbox secret of a demo store this node plays (testnet4 only).
    pub(crate) fn seller_mail_secret(&self, seller: &str) -> Option<StoreSecret> {
        if self.network != NetworkMode::Testnet4 {
            return None;
        }
        let secret = StoreSecret::from_bytes(fixture_store_keys(seller)?.mailbox).ok()?;
        (self.seller_mail_key(seller).ok()? == secret.public_key()).then_some(secret)
    }
}

/// One line per store: what a store list or a listing's store link needs.
pub(crate) fn store_summary(catalog: &Catalog, store: &Store, own: Option<&StoreId>) -> Result<Value, openbazaar_protocol::Error> {
    let profile = store.details.as_ref().map(|details| &details.profile);
    Ok(json!({
        "key": store.id.to_hex(),
        "name": store.name,
        "region": store.region,
        "tagline": profile.map_or("", |p| p.tagline.as_str()),
        "avatar": profile.and_then(|p| p.avatar).map(|hash| hex(&hash)),
        "contentRating": rating_name(store.rating),
        "acceptsXmr": store.details.as_ref().is_some_and(|details| details.accepts_xmr),
        "acceptsZec": store.details.as_ref().is_some_and(|details| details.accepts_zec),
        "listingCount": catalog.listings.iter().filter(|listing| listing.store_id == store.id).count(),
        "verified": store.verify().is_ok(),
        "own": own == Some(&store.id),
        "version": store.version(),
        "record": hex(&store.encode()?),
    }))
}

/// The full public profile, the store's listings, and its moderators.
pub(crate) fn store_detail(catalog: &Catalog, store: &Store, own: Option<&StoreId>) -> Result<Value, openbazaar_protocol::Error> {
    let mut summary = store_summary(catalog, store, own)?;
    let details = store.details.as_ref();
    let profile = details.map(|details| &details.profile);
    let fields: Map<String, Value> = [
        ("about", json!(profile.map_or("", |p| p.about.as_str()))),
        ("banner", json!(profile.and_then(|p| p.banner).map(|hash| hex(&hash)))),
        (
            "policies",
            json!({
                "shipping": profile.map_or("", |p| p.shipping_policy.as_str()),
                "returns": profile.map_or("", |p| p.returns_policy.as_str()),
                "processingTime": profile.map_or("", |p| p.processing_time.as_str()),
            }),
        ),
        ("shipsTo", json!(profile.map_or(Vec::new(), |p| p.ships_to.iter().map(|c| c.as_str().to_string()).collect()))),
        ("createdOn", json!(profile.map(|p| p.created_on))),
        ("mailboxKey", json!(details.map(|d| hex(&d.mailbox_key)))),
        ("onion", json!(details.map(|d| d.onion.as_str()).filter(|onion| !onion.is_empty()))),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    summary.as_object_mut().expect("summary is an object").extend(fields);
    let moderators: Vec<Value> = details
        .map(|d| d.moderators.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|id| {
            let known = *id == catalog.moderator.id && !catalog.moderator.name.is_empty();
            json!({ "id": hex(id), "name": known.then(|| catalog.moderator.name.clone()) })
        })
        .collect();
    let mut listings = Vec::new();
    for listing in catalog.listings.iter().filter(|listing| listing.store_id == store.id) {
        listings.push(listing_value(catalog, listing)?);
    }
    Ok(json!({ "store": summary, "listings": listings, "moderators": moderators }))
}

/// `GET /stores`: paired loopback callers only.
pub(crate) fn list_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let catalog = node.catalog();
    let stores: Result<Vec<Value>, _> = catalog
        .stores
        .iter()
        .map(|store| store_summary(&catalog, store, Some(&node.own_store)))
        .collect();
    match stores {
        Ok(stores) => json_body("200 OK", &json!({ "stores": stores }), origin),
        Err(err) => {
            eprintln!("ob-node: stores: {err}");
            json_error("500 Internal Server Error", "The stores could not be read.", origin)
        }
    }
}

/// `GET /stores/{key}`: paired loopback callers only.
pub(crate) fn detail_response(node: &Node, client: &Client, head: &str, key: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let Some(id) = unhex32(key.trim_end_matches('/')) else {
        return json_error("404 Not Found", "That store is not in this shop.", origin);
    };
    if let Err(err) = node.open_store_listings(id) {
        eprintln!("ob-node: open store: {err}");
    }
    let catalog = node.catalog();
    let Some(store) = catalog.store(&StoreId::from_bytes(id)) else {
        return json_error("404 Not Found", "That store is not in this shop.", origin);
    };
    match store_detail(&catalog, store, Some(&node.own_store)) {
        Ok(mut value) => {
            let pending = store.id == node.own_store && node.own_pending.load(Ordering::Relaxed);
            value["store"]["unpublishedChanges"] = json!(pending);
            json_body("200 OK", &value, origin)
        }
        Err(err) => {
            eprintln!("ob-node: store: {err}");
            json_error("500 Internal Server Error", "The store could not be read.", origin)
        }
    }
}
