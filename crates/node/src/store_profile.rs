//! The own store's storefront, edited from the paired client.
//!
//! `store-profile.json` holds what the seller typed; the signed store record
//! ([`crate::own_store`]) is built from it. Saving a profile signs the record
//! again, and signs every listing again when the store's name changed (each
//! listing names its seller). The role the user chose at onboarding, buyer
//! or seller, lives in the same file; it is not part of any signed record.

use openbazaar_protocol::{public_text, ContentRating, CountryCode, MAX_NAME};
use serde_json::{json, Map, Value};

use crate::own_listings::{capitalize, StoreError};
use crate::own_store::{read_profile, write_profile, PROFILE_FILE};
use crate::Node;

const TEXT_FIELDS: &[(&str, usize)] = &[
    ("name", MAX_NAME),
    ("region", 40),
    ("tagline", 120),
    ("about", 2000),
    ("shippingPolicy", 500),
    ("returnsPolicy", 500),
    ("processingTime", 120),
];
const IMAGE_FIELDS: &[&str] = &["avatar", "banner"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Buyer,
    Seller,
}

impl Role {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "buyer" => Some(Role::Buyer),
            "seller" => Some(Role::Seller),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Role::Buyer => "buyer",
            Role::Seller => "seller",
        }
    }
}

fn failed(err: impl std::fmt::Display) -> StoreError {
    StoreError::Failed(err.to_string())
}

/// The protocol's refusal of a profile field, as a sentence for the editor.
fn refusal(err: impl std::fmt::Display) -> StoreError {
    let text = err.to_string();
    StoreError::Invalid(capitalize(text.trim_start_matches(&format!("{PROFILE_FILE}: "))))
}

impl Node {
    /// The role chosen at onboarding, if any.
    pub(crate) fn role(&self) -> Option<Role> {
        read_profile(&self.data_dir).ok().and_then(|profile| profile["role"].as_str().and_then(Role::parse))
    }

    pub(crate) fn set_role(&self, role: Role) -> Result<Value, StoreError> {
        let mut profile = read_profile(&self.data_dir).map_err(failed)?;
        profile["role"] = json!(role.as_str());
        write_profile(&self.data_dir, &profile).map_err(failed)?;
        self.profile_value()
    }

    /// Everything the storefront editor shows.
    pub(crate) fn profile_value(&self) -> Result<Value, StoreError> {
        let profile = read_profile(&self.data_dir).map_err(failed)?;
        let record = self.own_record();
        let details = record.as_ref().and_then(|store| store.details.as_ref());
        let text = |key: &str| profile[key].as_str().unwrap_or("").to_string();
        let ships_to: Vec<Value> = profile["shipsTo"].as_array().cloned().unwrap_or_default();
        let listings = self.own_listings().entries.len();
        Ok(json!({
            "role": self.role().map(Role::as_str),
            "storeKey": self.own_store.to_hex(),
            "name": record.as_ref().map_or_else(|| text("name"), |store| store.name.clone()),
            "region": text("region"),
            "tagline": text("tagline"),
            "about": text("about"),
            "avatar": profile["avatar"].as_str(),
            "banner": profile["banner"].as_str(),
            "shippingPolicy": text("shippingPolicy"),
            "returnsPolicy": text("returnsPolicy"),
            "processingTime": text("processingTime"),
            "shipsTo": ships_to,
            "contentRating": crate::rating_name(record.as_ref().and_then(|store| store.rating)),
            "acceptsXmr": details.is_some_and(|d| d.accepts_xmr),
            "acceptsLightning": details.is_some_and(|d| d.accepts_lightning),
            // Monero and escrow sales need the seller's node to run those
            // rails for its own store; it does not yet.
            "xmrAvailable": false,
            "escrowAvailable": false,
            "published": record.is_some(),
            // The seller has saved the storefront once; until then it is the default.
            "setUp": profile["setUp"] == true,
            "unpublishedChanges": self.own_pending.load(std::sync::atomic::Ordering::Relaxed),
            "listingCount": listings,
            "network": self.network.as_str(),
        }))
    }

    /// Merge `body` into the profile, sign the store record again (and its
    /// listings, if the name changed), and serve it. Needs an unlocked wallet.
    pub(crate) fn update_profile(&self, body: &Value) -> Result<Value, StoreError> {
        let changes = parse_changes(body, |hash| self.media.get(hash).is_some())?;
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let old = read_profile(&self.data_dir).map_err(failed)?;
        let mut next = old.clone();
        for (key, value) in changes {
            next[key.as_str()] = value;
        }
        next["setUp"] = json!(true);
        write_profile(&self.data_dir, &next).map_err(failed)?;
        let record = self.own_record();
        let rating = record.as_ref().and_then(|store| store.rating).unwrap_or(ContentRating::General);
        if let Err(err) = self.publish_own(rating, crate::own_store::Rails::of(record.as_ref()), &secret) {
            write_profile(&self.data_dir, &old).map_err(failed)?;
            return Err(refusal(err));
        }
        self.resign_own_listings(&secret)?;
        self.profile_value()
    }
}

/// The fields `body` changes, checked. Unknown fields are refused.
fn parse_changes(body: &Value, media_has: impl Fn(&[u8; 32]) -> bool) -> Result<Map<String, Value>, StoreError> {
    let Some(map) = body.as_object() else {
        return Err(StoreError::Invalid("The storefront could not be read.".into()));
    };
    let mut out = Map::new();
    for (key, value) in map {
        if let Some((_, max)) = TEXT_FIELDS.iter().find(|(name, _)| name == key) {
            let text = value.as_str().ok_or_else(|| StoreError::Invalid(format!("The {key} must be text.")))?.trim();
            if text.contains('\0') {
                return Err(StoreError::Invalid(format!("The {key} is at most {max} characters.")));
            }
            // The signed record measures the display name in bytes.
            if key == "name" {
                if text.is_empty() {
                    return Err(StoreError::Invalid("Give the store a name.".into()));
                }
                if text.len() > MAX_NAME {
                    return Err(StoreError::Invalid(format!("The name is at most {MAX_NAME} characters.")));
                }
            } else if text.chars().count() > *max {
                return Err(StoreError::Invalid(format!("The {key} is at most {max} characters.")));
            }
            if key == "region" && !text.is_empty() {
                public_text::check_region("region", text).map_err(refusal)?;
            }
            out.insert(key.clone(), json!(text));
        } else if IMAGE_FIELDS.contains(&key.as_str()) {
            match value {
                Value::Null => {
                    out.insert(key.clone(), Value::Null);
                }
                Value::String(hash) => {
                    let bytes = crate::stores::unhex32(hash).ok_or_else(|| StoreError::Invalid(format!("The {key} is not an image hash.")))?;
                    if !media_has(&bytes) {
                        return Err(StoreError::Invalid(format!("Upload the {key} image first.")));
                    }
                    out.insert(key.clone(), json!(hash));
                }
                _ => return Err(StoreError::Invalid(format!("The {key} is not an image hash."))),
            }
        } else if key == "shipsTo" {
            let mut codes = Vec::new();
            for code in value.as_array().ok_or_else(|| StoreError::Invalid("Ships-to is a list of country codes.".into()))? {
                let code = code.as_str().unwrap_or("");
                CountryCode::parse(code).map_err(|_| StoreError::Invalid(format!("{code:?} is not a two-letter country code.")))?;
                if !codes.contains(&code.to_string()) {
                    codes.push(code.to_string());
                }
            }
            codes.sort();
            out.insert(key.clone(), json!(codes));
        } else {
            return Err(StoreError::Invalid("That field is not part of the storefront.".into()));
        }
    }
    Ok(out)
}
