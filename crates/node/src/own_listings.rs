//! Listings the node's own store signs: create, edit, delete.
//!
//! Each listing is a version 6 record with a random id that stays the same
//! across edits; every edit is a new signature over a new body, so the
//! listing's address (content hash) moves. The signed records are kept in
//! `own-listings.obv`, sealed with the vault key, together with the
//! addresses each listing had before its last edits, so an order a buyer
//! placed against an older version still finds the listing.
//!
//! Signing needs the store key, which is spend tier: while the wallet is
//! locked every change answers 423 and the served listings stay as they are.

use std::path::{Path, PathBuf};

use openbazaar_protocol::{
    Catalog, ContentLabel, ContentRating, ContentTags, FiatPrice, Listing, ListingDetails, ListingId, ListingKind,
    StoreId, MAX_IMAGES,
};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "own-listings.obv";
/// Older addresses remembered per listing.
const MAX_SUPERSEDED: usize = 16;
const MAX_LISTINGS: usize = 500;

/// Why a change to the own store's listings did not happen.
#[derive(Debug)]
pub(crate) enum StoreError {
    Invalid(String),
    NotFound,
    /// 423: needs the spend tier; the wallet is in this state.
    Locked(&'static str),
    Failed(String),
}

impl StoreError {
    fn invalid(message: impl Into<String>) -> Self {
        StoreError::Invalid(message.into())
    }
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub listing: Listing,
    /// Earlier addresses of this listing, newest first.
    pub superseded: Vec<String>,
}

pub(crate) struct OwnListings {
    path: PathBuf,
    vault: Vault,
    pub entries: Vec<Entry>,
}

impl OwnListings {
    /// Read the sealed file and keep only records the own store signed.
    pub(crate) fn open(dir: &Path, vault: Vault, own: &StoreId) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut entries = Vec::new();
        if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            for item in value["listings"].as_array().into_iter().flatten() {
                let record = unhex(item["record"].as_str().unwrap_or("")).map_err(Error::Detail)?;
                let listing = Listing::decode(&record).map_err(Error::Protocol)?;
                if listing.store_id != *own || listing.verify().is_err() || listing.id().is_none() {
                    eprintln!("ob-node: {FILE}: dropped a listing this store did not sign");
                    continue;
                }
                let superseded = item["superseded"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|hash| hash.as_str().map(str::to_string))
                    .collect();
                entries.push(Entry { listing, superseded });
            }
        }
        Ok(Self { path, vault, entries })
    }

    fn save(&self) -> Result<(), StoreError> {
        let mut items = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let record = entry.listing.encode().map_err(|err| StoreError::Failed(err.to_string()))?;
            items.push(json!({ "record": hex(&record), "superseded": entry.superseded }));
        }
        let bytes = serde_json::to_vec(&json!({ "v": 1, "listings": items })).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| StoreError::Failed(err.to_string()))
    }

    fn position(&self, id: &ListingId) -> Option<usize> {
        self.entries.iter().position(|entry| entry.listing.id() == Some(*id))
    }

    pub(crate) fn by_id(&self, id: &ListingId) -> Option<&Listing> {
        self.position(id).map(|index| &self.entries[index].listing)
    }

    pub(crate) fn current(&self) -> impl Iterator<Item = &Listing> {
        self.entries.iter().map(|entry| &entry.listing)
    }

    /// Keep `listing` as the new version of the listing with its id.
    pub(crate) fn supersede(&mut self, listing: Listing) -> Result<(), StoreError> {
        let index = listing.id().and_then(|id| self.position(&id)).ok_or(StoreError::NotFound)?;
        let before = self.entries[index].clone();
        let old_hash = before.listing.content_hash().map_err(|err| StoreError::Failed(err.to_string()))?.to_hex();
        let entry = &mut self.entries[index];
        entry.superseded.insert(0, old_hash);
        entry.superseded.truncate(MAX_SUPERSEDED);
        entry.listing = listing;
        if let Err(err) = self.save() {
            self.entries[index] = before;
            return Err(err);
        }
        Ok(())
    }

    /// The current version of the listing whose address is or was `hash`.
    pub(crate) fn by_hash(&self, hash: &str) -> Option<&Listing> {
        self.entries
            .iter()
            .find(|entry| {
                entry.listing.content_hash().is_ok_and(|current| current.to_hex() == hash)
                    || entry.superseded.iter().any(|old| old == hash)
            })
            .map(|entry| &entry.listing)
    }
}

/// What the editor sends, checked.
struct Draft {
    title: String,
    description: String,
    category: String,
    kind: ListingKind,
    ship_from: String,
    stock: u32,
    images: Vec<[u8; 32]>,
    pricing: Pricing,
    content: ContentLabel,
}

enum Pricing {
    Sats { price: u64, shipping: u64 },
    Fiat(FiatPrice),
}

const FIELDS: &[&str] = &[
    "title",
    "description",
    "category",
    "kind",
    "shipFrom",
    "stock",
    "images",
    "currency",
    "price",
    "shipping",
    "contentRating",
    "contentTags",
    "commerce",
];

fn text(body: &Value, key: &str, max: usize, required: bool) -> Result<String, StoreError> {
    let value = body[key].as_str().unwrap_or("").trim().to_string();
    if required && value.is_empty() {
        return Err(StoreError::invalid(format!("Enter a {}.", label(key))));
    }
    if value.chars().count() > max || value.contains('\0') {
        return Err(StoreError::invalid(format!("The {} is too long.", label(key))));
    }
    Ok(value)
}

fn label(key: &str) -> &str {
    match key {
        "shipFrom" => "ships-from region",
        other => other,
    }
}

fn amount(body: &Value, key: &str) -> Result<u64, StoreError> {
    match &body[key] {
        Value::Null => Ok(0),
        value => value.as_u64().ok_or_else(|| StoreError::invalid(format!("The {key} must be a whole number of the smallest unit."))),
    }
}

fn parse_draft(body: &Value, media_has: impl Fn(&[u8; 32]) -> bool) -> Result<Draft, StoreError> {
    let Some(map) = body.as_object() else {
        return Err(StoreError::invalid("The listing could not be read."));
    };
    if map.keys().any(|key| !FIELDS.contains(&key.as_str())) {
        return Err(StoreError::invalid("That field is not part of a listing."));
    }
    let kind = match body["kind"].as_str() {
        Some("physical") => ListingKind::Physical,
        Some("digital") => ListingKind::Digital,
        _ => return Err(StoreError::invalid("Choose a physical or a digital good.")),
    };
    let ship_from = text(body, "shipFrom", 40, kind == ListingKind::Physical)?;
    let stock = body["stock"]
        .as_u64()
        .filter(|stock| *stock <= 1_000_000)
        .ok_or_else(|| StoreError::invalid("Enter how many you have, from 0 to 1,000,000."))? as u32;
    let mut images = Vec::new();
    for image in body["images"].as_array().into_iter().flatten() {
        let hash = image.as_str().and_then(crate::stores::unhex32).ok_or_else(|| StoreError::invalid("An image is not a SHA-256 hash."))?;
        if !media_has(&hash) {
            return Err(StoreError::invalid("Upload each image before you add it to a listing."));
        }
        if !images.contains(&hash) {
            images.push(hash);
        }
    }
    if images.len() > MAX_IMAGES {
        return Err(StoreError::invalid(format!("A listing has at most {MAX_IMAGES} images.")));
    }
    let price = amount(body, "price")?;
    if price == 0 {
        return Err(StoreError::invalid("Enter a price above zero."));
    }
    let shipping = if kind == ListingKind::Physical { amount(body, "shipping")? } else { 0 };
    let pricing = match body["currency"].as_str().unwrap_or("SAT") {
        "SAT" => Pricing::Sats { price, shipping },
        code if crate::fiat::CURRENCIES.contains(&code) => {
            Pricing::Fiat(FiatPrice { currency: code.to_string(), amount: price, shipping })
        }
        _ => return Err(StoreError::invalid("Price it in sats, USD, EUR, GBP, or JPY.")),
    };
    let rating = match body["contentRating"].as_str() {
        None => ContentRating::General,
        Some(name) => ContentRating::from_name(name).ok_or_else(|| StoreError::invalid("The rating must be general, sensitive, or adult."))?,
    };
    let mut tags = ContentTags::empty();
    for name in body["contentTags"].as_array().into_iter().flatten() {
        let tag = name
            .as_str()
            .and_then(ContentTags::from_name)
            .ok_or_else(|| StoreError::invalid("Tags must be nudity, violence, drugs, weapons, or gambling."))?;
        tags = tags.with(tag);
    }
    Ok(Draft {
        title: text(body, "title", 120, true)?,
        description: text(body, "description", 5000, false)?,
        category: text(body, "category", 40, true)?,
        kind,
        ship_from: if kind == ListingKind::Physical { ship_from } else { String::new() },
        stock,
        images,
        pricing,
        content: ContentLabel::new(rating, tags),
    })
}

fn new_id() -> ListingId {
    let mut bytes = [0u8; 16];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    ListingId(bytes)
}

pub(crate) fn parse_id(text: &str) -> Option<ListingId> {
    crate::escrow_mail::parse_id(text.trim_end_matches('/')).map(ListingId)
}

impl Node {
    pub(crate) fn own_listings(&self) -> std::sync::MutexGuard<'_, OwnListings> {
        self.own_listings.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// The own store's listings as the editor shows them.
    pub(crate) fn own_listings_value(&self) -> Result<Value, StoreError> {
        let catalog = self.catalog();
        let mut out = Vec::new();
        for entry in &self.own_listings().entries {
            out.push(crate::listing_value(&catalog, &entry.listing).map_err(|err| StoreError::Failed(err.to_string()))?);
        }
        Ok(json!({ "listings": out }))
    }

    pub(crate) fn create_own_listing(&self, body: &Value) -> Result<Value, StoreError> {
        if self.own_listings().entries.len() >= MAX_LISTINGS {
            return Err(StoreError::invalid("This store already has the most listings a node signs."));
        }
        self.sign_own_listing(None, body)
    }

    pub(crate) fn update_own_listing(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let id = parse_id(id).ok_or(StoreError::NotFound)?;
        self.sign_own_listing(Some(id), body)
    }

    pub(crate) fn delete_own_listing(&self, id: &str) -> Result<(), StoreError> {
        let id = parse_id(id).ok_or(StoreError::NotFound)?;
        self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let mut own = self.own_listings();
        let index = own.position(&id).ok_or(StoreError::NotFound)?;
        let removed = own.entries.remove(index);
        if let Err(err) = own.save() {
            own.entries.insert(index, removed);
            return Err(err);
        }
        self.catalog_mut().listings.retain(|listing| !(listing.store_id == self.own_store && listing.id() == Some(id)));
        Ok(())
    }

    /// Sign a new listing (`id` is `None`) or a new version of one, keep it,
    /// and serve it in place of the old version.
    fn sign_own_listing(&self, id: Option<ListingId>, body: &Value) -> Result<Value, StoreError> {
        let draft = parse_draft(body, |hash| self.media.get(hash).is_some())?;
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let store = self.own_record().ok_or_else(|| StoreError::Failed("the own store has no signed record yet".into()))?;
        let (price_sats, shipping_sats, fiat) = match draft.pricing {
            Pricing::Sats { price, shipping } => (price, shipping, None),
            Pricing::Fiat(price) => {
                let no_rate =
                    || StoreError::invalid(format!("This node has no {} rate right now. Price it in sats, or try again.", price.currency));
                let rate = self.btc_price(&price.currency).ok_or_else(no_rate)?;
                let item = crate::fiat::sats_for(price.amount, &price.currency, rate).ok_or_else(no_rate)?;
                let shipping = match price.shipping {
                    0 => 0,
                    minor => crate::fiat::sats_for(minor, &price.currency, rate).ok_or_else(no_rate)?,
                };
                (item, shipping, Some(price))
            }
        };
        let mut own = self.own_listings();
        let previous = match id {
            Some(id) => Some(own.position(&id).ok_or(StoreError::NotFound)?),
            None => None,
        };
        let listed_on = previous.map_or_else(crate::own_store::today, |index| own.entries[index].listing.listed_on);
        let commerce = match &body["commerce"] {
            Value::Null => None,
            value => {
                let before = previous.and_then(|index| own.entries[index].listing.commerce.as_ref());
                Some(crate::commerce_draft::parse(value, before).map_err(StoreError::Invalid)?)
            }
        };
        if commerce.is_some() && fiat.is_none() {
            return Err(StoreError::invalid("Price a listing with options or shipping services in USD, EUR, GBP, or JPY."));
        }
        let stock = match &commerce {
            Some(commerce) if !commerce.variants.is_empty() => commerce.variants.total_stock().min(u64::from(u32::MAX)) as u32,
            _ => draft.stock,
        };
        let unsigned = Listing {
            store_id: self.own_store,
            title: draft.title,
            category: draft.category,
            price_sats,
            ship_from: draft.ship_from,
            description: draft.description,
            seller: store.name.clone(),
            stock,
            listed_on,
            moderated: false,
            kind: draft.kind,
            product: String::new(),
            shipping_sats,
            free_over_sats: 0,
            content: Some(draft.content),
            details: Some(ListingDetails { id: id.unwrap_or_else(new_id), images: draft.images, price: fiat }),
            commerce,
            signature: [0; 64],
        };
        let listing = unsigned.resign(&secret).map_err(|err| StoreError::invalid(capitalize(&err.to_string())))?;
        let entry = match previous {
            Some(index) => {
                let old = &own.entries[index];
                let mut superseded = vec![old.listing.content_hash().map_err(|err| StoreError::Failed(err.to_string()))?.to_hex()];
                superseded.extend(old.superseded.iter().cloned());
                superseded.truncate(MAX_SUPERSEDED);
                Entry { listing: listing.clone(), superseded }
            }
            None => Entry { listing: listing.clone(), superseded: Vec::new() },
        };
        let old = match previous {
            Some(index) => Some(std::mem::replace(&mut own.entries[index], entry)),
            None => {
                own.entries.push(entry);
                None
            }
        };
        if let Err(err) = own.save() {
            match (previous, old) {
                (Some(index), Some(old)) => own.entries[index] = old,
                _ => {
                    own.entries.pop();
                }
            }
            return Err(err);
        }
        drop(own);
        self.sync_shelf(old.as_ref().map(|entry| &entry.listing), &listing);
        self.serve_own_listing(listing.clone());
        crate::listing_value(&self.catalog(), &listing).map_err(|err| StoreError::Failed(err.to_string()))
    }

    pub(crate) fn serve_own_listing(&self, listing: Listing) {
        let mut catalog = self.catalog_mut();
        let id = listing.id();
        match catalog.listings.iter_mut().find(|old| old.store_id == listing.store_id && old.id().is_some() && old.id() == id) {
            Some(old) => *old = listing,
            None => catalog.listings.push(listing),
        }
    }

    /// Sign every own listing again with the store's current name, after
    /// the name in the store record changed.
    pub(crate) fn resign_own_listings(&self, secret: &bitcoin::secp256k1::SecretKey) -> Result<(), StoreError> {
        let Some(store) = self.own_record() else { return Ok(()) };
        let mut own = self.own_listings();
        let mut changed = Vec::new();
        for entry in own.entries.iter_mut() {
            if entry.listing.seller == store.name {
                continue;
            }
            let old_hash = entry.listing.content_hash().map_err(|err| StoreError::Failed(err.to_string()))?.to_hex();
            let listing = Listing { seller: store.name.clone(), ..entry.listing.clone() }
                .resign(secret)
                .map_err(|err| StoreError::invalid(capitalize(&err.to_string())))?;
            entry.superseded.insert(0, old_hash);
            entry.superseded.truncate(MAX_SUPERSEDED);
            entry.listing = listing.clone();
            changed.push(listing);
        }
        if changed.is_empty() {
            return Ok(());
        }
        own.save()?;
        drop(own);
        for listing in changed {
            self.serve_own_listing(listing);
        }
        Ok(())
    }

    /// Sign one own listing again with a new content label, from `POST /content-labels`.
    pub(crate) fn relabel_own_listing(&self, hash: &str, label: ContentLabel) -> Result<String, StoreError> {
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let mut own = self.own_listings();
        let index = own
            .entries
            .iter()
            .position(|entry| entry.listing.content_hash().is_ok_and(|current| current.to_hex() == hash))
            .ok_or(StoreError::NotFound)?;
        let old_hash = hash.to_string();
        let listing = own.entries[index].listing.relabel(label, &secret).map_err(|err| StoreError::Failed(err.to_string()))?;
        let new_hash = listing.content_hash().map_err(|err| StoreError::Failed(err.to_string()))?.to_hex();
        let entry = &mut own.entries[index];
        entry.superseded.insert(0, old_hash);
        entry.superseded.truncate(MAX_SUPERSEDED);
        entry.listing = listing.clone();
        own.save()?;
        drop(own);
        self.serve_own_listing(listing);
        Ok(new_hash)
    }
}

/// Put the own store's listings into `catalog` at boot.
pub(crate) fn serve_at_boot(catalog: &mut Catalog, own: &OwnListings) {
    catalog.listings.extend(own.entries.iter().map(|entry| entry.listing.clone()));
}

/// A protocol refusal as a sentence for the editor.
pub(crate) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(chars.as_str());
            if !out.ends_with('.') {
                out.push('.');
            }
            out
        }
        None => String::new(),
    }
}
