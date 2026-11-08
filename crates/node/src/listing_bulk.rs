//! One price, or one availability, applied to listings the seller selects.
//!
//! Each selected listing is signed by the same path as a single edit. The
//! batch is saved only after every selected listing signs. A draft is not a
//! selected listing and is left unpublished.

use serde_json::{json, Value};

use crate::own_listings::{parse_id, StoreError};
use crate::Node;

impl Node {
    /// `{"ids":[...],"price":n}` or `{"ids":[...],"availability":{...}}`.
    pub(crate) fn bulk_edit_listings(&self, body: &Value) -> Result<Value, StoreError> {
        let ids = listing_ids(&body["ids"])?;
        let change = change_of(body)?;
        let mut currents = Vec::with_capacity(ids.len());
        {
            let own = self.own_listings();
            for id in &ids {
                let listing = own.by_id(id).ok_or(StoreError::NotFound)?.clone();
                currents.push((*id, listing));
            }
        }
        let mut signed = Vec::with_capacity(currents.len());
        for (id, current) in &currents {
            let mut draft = self.editor_body(current)?;
            apply(&mut draft, &change)?;
            signed.push((*id, self.signed_listing(Some(*id), &draft, Some(current))?));
        }
        let previous = self.own_listings().replace_all(&signed)?;
        let mut out = Vec::with_capacity(signed.len());
        for ((_, listing), before) in signed.into_iter().zip(previous) {
            self.sync_shelf(Some(&before), &listing);
            let listing = if self.on_vacation() && !crate::vacation::is_paused(&listing) {
                let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
                self.note_vacation_availability(&listing)?;
                self.publish_paused(&listing, &secret)?
            } else {
                self.serve_own_listing(listing.clone());
                listing
            };
            out.push(crate::listing_value(&self.catalog(), &listing).map_err(|err| StoreError::Failed(err.to_string()))?);
        }
        Ok(json!({ "listings": out }))
    }
}

fn listing_ids(value: &Value) -> Result<Vec<openbazaar_protocol::ListingId>, StoreError> {
    let items = value.as_array().ok_or_else(|| StoreError::invalid("Select a listing."))?;
    if items.is_empty() {
        return Err(StoreError::invalid("Select a listing."));
    }
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let id = item.as_str().and_then(parse_id).ok_or(StoreError::NotFound)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

enum Change {
    Price(u64),
    Availability(Value),
}

fn change_of(body: &Value) -> Result<Change, StoreError> {
    let price = body.get("price");
    let availability = body.get("availability");
    match (price, availability) {
        (Some(price), None) => {
            let amount = price.as_u64().ok_or_else(|| StoreError::invalid("The price must be a whole number of the smallest unit."))?;
            if amount == 0 {
                return Err(StoreError::invalid("Enter a price above zero."));
            }
            Ok(Change::Price(amount))
        }
        (None, Some(availability)) => Ok(Change::Availability(availability.clone())),
        _ => Err(StoreError::invalid("Change the price or the availability.")),
    }
}

fn apply(draft: &mut Value, change: &Change) -> Result<(), StoreError> {
    match change {
        Change::Price(amount) => {
            draft["price"] = json!(amount);
            Ok(())
        }
        Change::Availability(availability) => {
            if !draft["commerce"].is_object() {
                return Err(StoreError::invalid("This listing has no availability to change."));
            }
            draft["commerce"]["availability"] = availability.clone();
            Ok(())
        }
    }
}
