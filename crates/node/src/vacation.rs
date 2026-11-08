//! Vacation mode: a store-level pause that is not part of the signed store record.
//!
//! The flag lives in `store-profile.json`. While it is on, every own listing
//! is signed again with stock 0 and, when the listing has a commerce block,
//! `away` availability. The stock ledger keeps the shelf count, and the
//! availability to restore is kept next to the flag. Orders are declined
//! with one fixed sentence. There is no free-text note: a note could say
//! where the seller is. Nothing here is told to a list of buyers.

use std::collections::BTreeMap;

use bitcoin::secp256k1::SecretKey;
use openbazaar_protocol::{Availability, Listing, ListingId};
use serde_json::{json, Value};

use crate::own_listings::{capitalize, parse_id, StoreError};
use crate::own_store::{read_profile, write_profile};
use crate::Node;

/// What a buyer is told when the store is paused. Not a place or a date.
pub(crate) const AWAY_MESSAGE: &str = "The store is away and not taking orders.";

const SNAPS: &str = "vacationAvailability";

fn failed(err: impl std::fmt::Display) -> StoreError {
    StoreError::Failed(err.to_string())
}

/// The signed listing buyers (and replicas) should see while the store is paused.
pub(crate) fn paused(mut listing: Listing) -> Listing {
    listing.stock = 0;
    if let Some(commerce) = listing.commerce.as_mut() {
        commerce.purchase.availability = Availability::Away;
        for variant in &mut commerce.variants.variants {
            variant.stock = 0;
        }
    }
    listing
}

/// Stock 0, and `away` when the listing has commerce. A sold-out listing is not paused.
pub(crate) fn is_paused(listing: &Listing) -> bool {
    if listing.stock != 0 {
        return false;
    }
    match &listing.commerce {
        None => true,
        Some(commerce) => {
            commerce.purchase.availability == Availability::Away
                && commerce
                    .variants
                    .variants
                    .iter()
                    .all(|variant| variant.stock == 0)
        }
    }
}

fn availability_value(availability: Availability) -> Value {
    let (tag, value) = match availability {
        Availability::InStock => (0, 0u32),
        Availability::PreOrder { ships_on } => (1, ships_on),
        Availability::MadeToOrder { days } => (2, u32::from(days)),
        Availability::Away => (3, 0),
    };
    json!({ "tag": tag, "value": value })
}

fn availability_of(item: &Value) -> Option<Availability> {
    let tag = item["tag"].as_u64()?;
    let value = u32::try_from(item["value"].as_u64()?).ok()?;
    match tag {
        0 => Some(Availability::InStock),
        1 => Some(Availability::PreOrder { ships_on: value }),
        2 => {
            let days = u16::try_from(value).ok()?;
            Some(Availability::MadeToOrder { days })
        }
        3 => Some(Availability::Away),
        _ => None,
    }
}

impl Node {
    pub(crate) fn on_vacation(&self) -> bool {
        read_profile(&self.data_dir).is_ok_and(|profile| profile["away"] == true)
    }

    /// `{"away": true|false}` and nothing else. A note is refused.
    pub(crate) fn set_vacation(&self, body: &Value) -> Result<Value, StoreError> {
        let away = parse_away(body)?;
        if away == self.on_vacation() {
            return self.profile_value();
        }
        let secret = self
            .own_signing_key()
            .ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        if away {
            self.begin_vacation(&secret)?;
        } else {
            self.end_vacation(&secret)?;
        }
        self.profile_value()
    }

    /// Remember this listing's availability the first time we pause it.
    /// A later edit that still says `away` must not replace that memory.
    pub(crate) fn note_vacation_availability(&self, listing: &Listing) -> Result<(), StoreError> {
        let Some(id) = listing.id() else {
            return Ok(());
        };
        let Some(commerce) = &listing.commerce else {
            return Ok(());
        };
        let mut profile = read_profile(&self.data_dir).map_err(failed)?;
        let mut snaps = profile[SNAPS].as_array().cloned().unwrap_or_default();
        if snaps.iter().any(|item| {
            item["id"]
                .as_str()
                .is_some_and(|text| parse_id(text) == Some(id))
        }) {
            return Ok(());
        }
        let mut item = availability_value(commerce.purchase.availability);
        item["id"] = json!(id.to_hex());
        snaps.push(item);
        profile[SNAPS] = json!(snaps);
        write_profile(&self.data_dir, &profile).map_err(failed)
    }

    /// Sign `listing` as paused and serve that. Does not touch the stock ledger.
    pub(crate) fn publish_paused(
        &self,
        listing: &Listing,
        secret: &SecretKey,
    ) -> Result<Listing, StoreError> {
        let paused = paused(listing.clone())
            .resign(secret)
            .map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        self.own_listings().supersede(paused.clone())?;
        self.serve_own_listing(paused.clone());
        Ok(paused)
    }

    fn begin_vacation(&self, secret: &SecretKey) -> Result<(), StoreError> {
        let listings: Vec<Listing> = self.own_listings().current().cloned().collect();
        for listing in &listings {
            self.note_vacation_availability(listing)?;
            self.pin_shelf(listing);
        }
        for listing in &listings {
            if is_paused(listing) {
                continue;
            }
            self.publish_paused(listing, secret)?;
        }
        self.write_away(true)
    }

    fn end_vacation(&self, secret: &SecretKey) -> Result<(), StoreError> {
        let snaps = self.vacation_snaps()?;
        let listings: Vec<Listing> = self.own_listings().current().cloned().collect();
        for listing in listings {
            let mut next = crate::live_stock::listing_at_shelf(&listing, |key, signed| {
                self.stock().shelf(key, signed)
            });
            if let Some(id) = next.id() {
                if let Some(availability) = snaps.get(&id) {
                    if let Some(commerce) = next.commerce.as_mut() {
                        commerce.purchase.availability = *availability;
                    }
                }
            }
            if next.stock == listing.stock && next.commerce == listing.commerce {
                continue;
            }
            let signed = Listing {
                signature: [0; 64],
                ..next
            }
            .resign(secret)
            .map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
            self.own_listings().supersede(signed.clone())?;
            self.serve_own_listing(signed);
        }
        self.write_away(false)
    }

    fn vacation_snaps(&self) -> Result<BTreeMap<ListingId, Availability>, StoreError> {
        let profile = read_profile(&self.data_dir).map_err(failed)?;
        let mut out = BTreeMap::new();
        for item in profile[SNAPS].as_array().into_iter().flatten() {
            let Some(id) = item["id"].as_str().and_then(parse_id) else {
                continue;
            };
            let Some(availability) = availability_of(item) else {
                continue;
            };
            out.insert(id, availability);
        }
        Ok(out)
    }

    fn write_away(&self, away: bool) -> Result<(), StoreError> {
        let mut profile = read_profile(&self.data_dir).map_err(failed)?;
        profile["away"] = json!(away);
        if !away {
            if let Some(map) = profile.as_object_mut() {
                map.remove(SNAPS);
            }
        }
        write_profile(&self.data_dir, &profile).map_err(failed)
    }
}

fn parse_away(body: &Value) -> Result<bool, StoreError> {
    let Some(map) = body.as_object() else {
        return Err(StoreError::Invalid("Vacation mode is on or off.".into()));
    };
    if map.len() != 1 || !map.contains_key("away") {
        return Err(StoreError::Invalid("Vacation mode is on or off.".into()));
    }
    map["away"]
        .as_bool()
        .ok_or_else(|| StoreError::Invalid("Vacation mode is on or off.".into()))
}
