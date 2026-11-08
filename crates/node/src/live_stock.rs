//! Keeps the own store's signed listings in step with the stock ledger.
//!
//! A signed listing shows the shelf count as of its last publish: what the
//! seller set, less what has sold. Units held for unpaid quotes are not
//! taken off it, since a hold lasts an hour. When a sale is paid its units
//! leave the shelf, and the next watcher round signs the listing again (only
//! while the store key is unlocked; otherwise on a later round). When the
//! seller edits a variant's stock, that count replaces the ledger's.
//!
//! A paid sale also gets one store-signed receipt per listing for the
//! order's review key, so the buyer can later review it as a verified
//! purchase without linking their other orders.

use openbazaar_protocol::{Listing, ListingId, Receipt, SignedReceipt, VariantId};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::sale_terms::signed_stock;
use crate::stock::{Key, Refused, Settled};
use crate::Node;

/// The signed stock of `variant`, if the listing has it.
fn stock_of(listing: &Listing, variant: VariantId) -> Option<u32> {
    match listing.commerce.as_ref().filter(|c| !c.variants.is_empty()) {
        Some(commerce) => commerce.variants.get(variant).map(|found| found.stock),
        None => (variant == VariantId::NONE).then_some(listing.stock),
    }
}

/// Every stock key of a listing and its signed count.
fn keys_of(listing: &Listing) -> Vec<(Key, u32)> {
    let Some(id) = listing.id() else { return Vec::new() };
    match listing.commerce.as_ref().filter(|c| !c.variants.is_empty()) {
        Some(commerce) => commerce.variants.variants.iter().map(|v| ((id, v.id), v.stock)).collect(),
        None => vec![((id, VariantId::NONE), listing.stock)],
    }
}

fn line_key(line: &Value) -> Option<(Key, u32)> {
    let listing: [u8; 16] = unhex(line["listingId"].as_str()?).ok()?.try_into().ok()?;
    let variant: [u8; 8] = match line["variant"].as_str() {
        Some(text) => unhex(text).ok()?.try_into().ok()?,
        None => VariantId::NONE.0,
    };
    Some(((ListingId(listing), VariantId(variant)), line["quantity"].as_u64()? as u32))
}

impl Node {
    /// A sale was just paid: sell its units and sign its receipts. Returns
    /// the fields to add to the "paid" reply and to the sale.
    pub(crate) fn settle_paid_sale(&self, sale: &Value) -> Value {
        let id = sale["id"].as_str().unwrap_or("");
        let lines: Vec<(Key, u32)> = sale["lines"].as_array().into_iter().flatten().filter_map(line_key).collect();
        let mut short = Vec::new();
        let mut stock = self.stock();
        if sale["quote"].is_null() {
            let own = self.own_listings();
            for (key, quantity) in &lines {
                let signed = own.by_id(&key.0).map_or(0, |listing| signed_stock(listing, key.1));
                match stock.reserve(id, *key, *quantity, signed, 0) {
                    Ok(()) => {}
                    Err(Refused::Short { .. }) => short.push(*key),
                    Err(Refused::Failed(err)) => eprintln!("ob-node: sale {id}: {err}"),
                }
            }
        }
        let settled = stock.settle(id).unwrap_or_else(|err| {
            eprintln!("ob-node: sale {id}: {err}");
            Vec::new()
        });
        drop(stock);
        short.extend(settled.iter().filter(|(_, settled)| *settled == Settled::Oversold).map(|(key, _)| *key));
        let oversold: Vec<Value> = short.iter().map(|key| json!({ "listingId": key.0.to_hex(), "variant": hex(&key.1 .0) })).collect();
        let mut extra = json!({ "receipts": self.receipts_for(sale) });
        if !oversold.is_empty() {
            extra["oversold"] = json!(oversold);
        }
        extra
    }

    /// The "paid" reply for a sale that just became paid, after its units
    /// are sold. Marks the sale oversold or its receipts pending as needed.
    pub(crate) fn paid_reply(&self, sale: &Value) -> Value {
        let extra = self.settle_paid_sale(sale);
        let mut reply = json!({ "v": 1, "kind": "paid", "txids": sale["txids"] });
        let pending = extra["receipts"].is_null();
        if !pending && extra["receipts"].as_array().is_some_and(|r| !r.is_empty()) {
            reply["receipts"] = extra["receipts"].clone();
        }
        if !extra["oversold"].is_null() {
            reply["oversold"] = extra["oversold"].clone();
        }
        if pending || !extra["oversold"].is_null() {
            let id = sale["id"].as_str().unwrap_or("");
            let result = self.sales().update(id, |sale| {
                if pending {
                    sale["receiptsPending"] = json!(true);
                }
                if !extra["oversold"].is_null() {
                    sale["oversold"] = extra["oversold"].clone();
                }
            });
            if let Err(err) = result {
                eprintln!("ob-node: sale {id}: {err}");
            }
        }
        reply
    }

    /// `reply` with the receipts a sale could not get when it was paid, if the key is unlocked now.
    pub(crate) fn with_pending_receipts(&self, sale: &Value, mut reply: Value) -> Value {
        if sale["receiptsPending"] != true {
            return reply;
        }
        let receipts = self.receipts_for(sale);
        if receipts.as_array().is_some_and(|r| !r.is_empty()) {
            reply["receipts"] = receipts;
            let id = sale["id"].as_str().unwrap_or("");
            if let Err(err) = self.sales().update(id, |sale| sale["receiptsPending"] = json!(false)) {
                eprintln!("ob-node: sale {id}: {err}");
            }
        }
        reply
    }

    /// One receipt per listing in `sale`, or null while the store key is locked
    /// or the order named no review key.
    pub(crate) fn receipts_for(&self, sale: &Value) -> Value {
        let Some(review_key) = sale["quote"]["reviewKey"].as_str().and_then(crate::stores::unhex32) else {
            return json!([]);
        };
        let Some(secret) = self.own_signing_key() else { return Value::Null };
        let mut ids: Vec<ListingId> = sale["lines"].as_array().into_iter().flatten().filter_map(line_key).map(|(key, _)| key.0).collect();
        ids.sort();
        ids.dedup();
        let paid_on = crate::reviews::paid_month(crate::own_store::today());
        let receipts: Vec<String> = ids
            .into_iter()
            .filter_map(|listing_id| {
                let receipt = Receipt { store_id: self.own_store, listing_id, review_key, paid_on };
                SignedReceipt::sign(receipt, &secret).and_then(|signed| signed.encode()).ok().map(|bytes| hex(&bytes))
            })
            .collect();
        json!(receipts)
    }

    /// Remember the shelf count before a pause publishes stock 0.
    /// `shelf` falls back to the signed count, so this has to run first.
    pub(crate) fn pin_shelf(&self, listing: &Listing) {
        let mut stock = self.stock();
        for (key, signed) in keys_of(listing) {
            let count = stock.shelf(&key, signed);
            if let Err(err) = stock.set_on_hand(key, count) {
                eprintln!("ob-node: stock: {err}");
            }
        }
    }

    /// The seller saved a listing: the stock they changed replaces the ledger's count.
    pub(crate) fn sync_shelf(&self, before: Option<&Listing>, after: &Listing) {
        let mut stock = self.stock();
        for (key, count) in keys_of(after) {
            if before.and_then(|b| stock_of(b, key.1)) == Some(count) {
                continue;
            }
            if let Err(err) = stock.set_on_hand(key, count) {
                eprintln!("ob-node: stock: {err}");
            }
        }
    }

    /// One watcher round: release lapsed holds, then sign again every own
    /// listing whose shelf moved. Returns how many were signed.
    pub(crate) fn watch_stock(&self) -> usize {
        if let Err(err) = self.stock().sweep(crate::escrow_outbox::now_secs()) {
            eprintln!("ob-node: stock: {err}");
        }
        // A pause publishes stock 0 on purpose. Republishing the shelf would undo it.
        if self.on_vacation() {
            return 0;
        }
        let Some(secret) = self.own_signing_key() else { return 0 };
        let moved: Vec<Listing> = {
            let own = self.own_listings();
            let stock = self.stock();
            own.current().filter_map(|listing| restocked(listing, |key, signed| stock.shelf(key, signed))).collect()
        };
        let mut signed = 0;
        for listing in moved {
            let result = listing.resign(&secret).map_err(|err| err.to_string()).and_then(|listing| {
                self.own_listings().supersede(listing.clone()).map_err(|err| format!("{err:?}"))?;
                Ok(listing)
            });
            match result {
                Ok(listing) => {
                    self.serve_own_listing(listing);
                    signed += 1;
                }
                Err(err) => eprintln!("ob-node: stock: {err}"),
            }
        }
        signed
    }
}

/// `listing` with its stock set from the ledger. The same listing when nothing moved.
pub(crate) fn listing_at_shelf(listing: &Listing, shelf: impl Fn(&Key, u32) -> u32) -> Listing {
    restocked(listing, shelf).unwrap_or_else(|| listing.clone())
}

/// `listing` with its stock set from `shelf`, or `None` if nothing moved.
fn restocked(listing: &Listing, shelf: impl Fn(&Key, u32) -> u32) -> Option<Listing> {
    let counts: Vec<(Key, u32, u32)> = keys_of(listing).into_iter().map(|(key, signed)| (key, signed, shelf(&key, signed))).collect();
    if counts.iter().all(|(_, signed, now)| signed == now) {
        return None;
    }
    let mut next = listing.clone();
    match next.commerce.as_mut().filter(|c| !c.variants.is_empty()) {
        Some(commerce) => {
            for variant in commerce.variants.variants.iter_mut() {
                if let Some((_, _, now)) = counts.iter().find(|(key, _, _)| key.1 == variant.id) {
                    variant.stock = *now;
                }
            }
            next.stock = commerce.variants.total_stock().min(u64::from(u32::MAX)) as u32;
        }
        None => next.stock = counts[0].2,
    }
    Some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbazaar_protocol::Commerce;

    fn tee() -> Listing {
        let mut listing = openbazaar_protocol::fixture_catalog().listings.into_iter().next().unwrap();
        let draft = json!({
            "axes": [{ "name": "Size", "values": ["S", "M"] }],
            "variants": [{ "options": ["S"], "stock": 4 }, { "options": ["M"], "stock": 2 }],
        });
        let commerce: Commerce = crate::commerce_draft::parse(&draft, None).unwrap();
        listing.details = Some(openbazaar_protocol::ListingDetails { id: ListingId([9; 16]), images: vec![], price: None });
        listing.stock = 6;
        listing.commerce = Some(commerce);
        listing
    }

    #[test]
    fn a_sale_moves_the_variant_and_the_total() {
        let listing = tee();
        assert!(restocked(&listing, |_, signed| signed).is_none());
        let m = listing.commerce.as_ref().unwrap().variants.variants[1].id;
        let next = restocked(&listing, |key, signed| if key.1 == m { 1 } else { signed }).unwrap();
        assert_eq!(stock_of(&next, m), Some(1));
        assert_eq!(next.stock, 5);
    }

    #[test]
    fn only_changed_or_new_variants_reset_the_shelf() {
        let listing = tee();
        let keys = keys_of(&listing);
        assert_eq!(keys.len(), 2);
        assert_eq!(stock_of(&listing, VariantId::NONE), None, "a listing with variants has no plain stock key");
        assert_eq!(stock_of(&listing, keys[0].0 .1), Some(4));
    }
}
