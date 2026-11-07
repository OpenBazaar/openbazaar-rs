//! Version 3 order terms on the buyer's side: variants, the shipping
//! service and a coupon code for a package to a store on another node.
//!
//! The buyer's node prices the package with the same rules the seller's
//! node will ([`openbazaar_protocol::pricing::quote`]) and keeps that quote.
//! The seller's terms may come in lower (a coupon) but never higher, and the
//! sats they ask may move with the rate as for any fiat price. A receipt in
//! the "paid" or "shipped" reply is kept once it checks out: signed by the
//! store, for a listing in the order, naming this order's review key.

use openbazaar_protocol::pricing::{quote, At, Delivery, Line};
use openbazaar_protocol::{Listing, ListingKind, SignedReceipt, VariantId};
use serde_json::{json, Value};
use tormail::{ListingHash, OrderTerms, TermsLine, NO_SERVICE};

use crate::escrow_mail::{hex, unhex};
use crate::{Node, OrderError};

pub(crate) const LINE_TERMS_FIELD: &str = "variant";

/// What a package asks beyond listings and quantities.
pub(crate) struct TermsPlan {
    pub variants: Vec<VariantId>,
    pub service: u8,
    pub coupon: String,
}

/// The package's terms, if it needs version 3: a listing with commerce
/// terms, a variant, a service or a coupon.
pub(crate) fn plan(package: &Value, lines: &[(&Listing, u32)]) -> Result<Option<TermsPlan>, OrderError> {
    let bad = || OrderError::Invalid("Choose an option the listing offers.");
    let mut variants = Vec::with_capacity(lines.len());
    for (i, _) in lines.iter().enumerate() {
        let variant = match package["lines"].get(i).and_then(|line| line.get(LINE_TERMS_FIELD)) {
            None | Some(Value::Null) => VariantId::NONE,
            Some(value) => VariantId(unhex(value.as_str().ok_or_else(bad)?).ok().and_then(|b| b.try_into().ok()).ok_or_else(bad)?),
        };
        variants.push(variant);
    }
    let service = match &package["service"] {
        Value::Null => 0,
        Value::String(text) if text == "pickup" => NO_SERVICE,
        value => value.as_u64().filter(|n| *n < u64::from(NO_SERVICE)).ok_or(OrderError::Invalid("Choose a shipping service."))? as u8,
    };
    let coupon = package["coupon"].as_str().unwrap_or("").trim().to_string();
    if coupon.len() > 24 {
        return Err(OrderError::Invalid("That coupon code is too long."));
    }
    let needed = lines.iter().any(|(listing, _)| listing.commerce.is_some())
        || variants.iter().any(|v| *v != VariantId::NONE)
        || service != 0
        || !coupon.is_empty();
    Ok(needed.then_some(TermsPlan { variants, service, coupon }))
}

fn delivery(listing: &Listing, service: u8) -> Delivery {
    match (listing.kind, service) {
        (ListingKind::Digital, _) => Delivery::Digital,
        (ListingKind::Physical, NO_SERVICE) => Delivery::Pickup,
        (ListingKind::Physical, index) => Delivery::Service(index),
    }
}

impl Node {
    /// The package priced in its fiat currency, without a coupon (only the
    /// seller knows its codes), and in sats at this node's rate now.
    pub(crate) fn terms_estimate(&self, lines: &[(&Listing, u32)], plan: &TermsPlan) -> Result<(u64, u64, Value), OrderError> {
        let priced: Vec<Line> = lines
            .iter()
            .zip(&plan.variants)
            .map(|((listing, quantity), variant)| Line {
                listing,
                variant: *variant,
                quantity: *quantity,
                delivery: delivery(listing, plan.service),
                offer: None,
            })
            .collect();
        let at = At { today: crate::own_store::today(), now: crate::escrow_outbox::now_secs(), buyer_key: None, country: None };
        let quote = quote(&priced, None, at).map_err(|err| OrderError::Invalid(err.as_str()))?;
        let rate = self.btc_price(&quote.currency).ok_or(OrderError::Invalid("This node has no exchange rate right now. Try again."))?;
        let sats = |minor: u64| if minor == 0 { Some(0) } else { crate::fiat::sats_for(minor, &quote.currency, rate) };
        let (Some(items), Some(shipping)) = (sats(quote.items), sats(quote.shipping)) else {
            return Err(OrderError::Invalid("The order total is too small or too large to pay."));
        };
        let value = json!({ "currency": quote.currency, "items": quote.items, "shipping": quote.shipping, "total": quote.total });
        Ok((items, shipping, value))
    }

    /// The version 3 terms for one package's letter.
    pub(crate) fn letter_terms(&self, checkout_id: &str, store: &[u8; 32], lines: &[(&Listing, u32)], plan: &TermsPlan) -> Result<OrderTerms, String> {
        let mut terms_lines = Vec::with_capacity(lines.len());
        for ((listing, quantity), variant) in lines.iter().zip(&plan.variants) {
            let hash = listing.content_hash().map_err(|err| err.to_string())?;
            terms_lines.push(TermsLine { listing_hash: ListingHash(*hash.as_bytes()), variant_id: variant.0, quantity: *quantity, offer: Vec::new() });
        }
        Ok(OrderTerms {
            lines: terms_lines,
            service: plan.service,
            coupon: plan.coupon.clone().into_bytes(),
            review_key: self.order_review_key(checkout_id, store)?,
        })
    }

    pub(crate) fn order_review_key(&self, checkout_id: &str, store: &[u8; 32]) -> Result<[u8; 32], String> {
        let secret = self.order_review_secret(checkout_id, store)?;
        let secp = bitcoin::secp256k1::Secp256k1::new();
        Ok(bitcoin::secp256k1::Keypair::from_secret_key(&secp, &secret).x_only_public_key().0.serialize())
    }
}

/// Why the seller's fiat quote is not acceptable, if it is not.
pub(crate) fn quote_problem(remote: &Value, body: &Value) -> Option<String> {
    let ours = &remote["quote"];
    if ours.is_null() {
        return None;
    }
    let theirs = &body["quote"];
    let (Some(total), Some(limit)) = (theirs["total"].as_u64(), ours["total"].as_u64()) else {
        return Some("The seller's terms did not include the price in the listing's currency. Nothing was paid.".into());
    };
    if theirs["currency"] != ours["currency"] || total > limit {
        return Some("The seller asked more than the listing's price at checkout. Nothing was paid.".into());
    }
    None
}

/// Keep the receipts and any oversold notice from a "paid" or "shipped" reply.
pub(crate) fn keep_receipts(order: &mut crate::buyer_orders::BuyerOrder, body: &Value, review_key: Option<[u8; 32]>) {
    let Some(remote) = order.remote.as_mut() else { return };
    if body["oversold"].is_array() {
        remote["oversold"] = body["oversold"].clone();
    }
    let Some(review_key) = review_key else { return };
    let store = order.store_id.clone();
    let listings: Vec<String> = order.lines.iter().filter_map(|line| line["listingId"].as_str().map(str::to_string)).collect();
    let mut kept: Vec<Value> = remote["receipts"].as_array().cloned().unwrap_or_default();
    for text in body["receipts"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some(receipt) = unhex(text).ok().and_then(|bytes| SignedReceipt::decode(&bytes).ok()) else { continue };
        let terms = &receipt.terms;
        let fits = receipt.verify().is_ok()
            && terms.store_id.to_hex() == store
            && terms.review_key == review_key
            && listings.contains(&terms.listing_id.to_hex());
        if fits && !kept.iter().any(|k| k == text) {
            kept.push(json!(text));
        }
    }
    if !kept.is_empty() {
        remote["receipts"] = json!(kept);
    }
}

/// Name each order line's listing id, and its variant if it has one.
pub(crate) fn annotate_lines(values: &mut [Value], lines: &[(&Listing, u32)], plan: Option<&TermsPlan>) {
    for (i, (value, (listing, _))) in values.iter_mut().zip(lines).enumerate() {
        value["listingId"] = json!(listing.id().map(|id| id.to_hex()));
        let Some(variant) = plan.and_then(|plan| plan.variants.get(i)).filter(|v| **v != VariantId::NONE) else { continue };
        value["variant"] = json!(hex(&variant.0));
        if let Some(commerce) = &listing.commerce {
            if let Some(found) = commerce.variants.get(*variant) {
                let label: Vec<String> = commerce.variants.describe(found).into_iter().map(|(_, v)| v).collect();
                value["variantLabel"] = json!(label.join(" / "));
            }
        }
    }
}
