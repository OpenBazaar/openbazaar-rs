//! A version 3 order letter on the seller's node: lines with variants, one
//! shipping service, a coupon code and accepted offers.
//!
//! The package is priced in the listings' fiat currency with
//! [`openbazaar_protocol::pricing::quote`], the rules the buyer's client ran,
//! then converted to sats at the node's rate now. The units are held in the
//! stock ledger for the payment window before the seller replies; a letter
//! that asks for more than is left is declined before anything is paid.

use openbazaar_protocol::{Availability, Listing, ListingKind, VariantId};
use serde_json::{json, Value};
use tormail::{Order, TermsLine};

use crate::escrow_mail::hex;
use crate::order_mail::MailError;
use crate::order_terms::{Checked, TermsLedger};
use crate::sales::Priced;
use crate::stock::{Key, Refused, StockLedger};
use crate::Node;

/// How long a quote's units stay held without a payment.
pub(crate) const PAYMENT_WINDOW: u64 = 60 * 60;

/// Units in the signed listing for one variant (the listing's own stock if it has none).
pub(crate) fn signed_stock(listing: &Listing, variant: VariantId) -> u32 {
    match listing.commerce.as_ref().and_then(|c| c.variants.get(variant)) {
        Some(found) => found.stock,
        None => listing.stock,
    }
}

/// Made to order with no stock set: there is no shelf to run out.
fn unlimited(listing: &Listing, variant: VariantId) -> bool {
    let made = listing.commerce.as_ref().is_some_and(|c| matches!(c.purchase.availability, Availability::MadeToOrder { .. }));
    made && signed_stock(listing, variant) == 0
}

fn line_value(listing: &Listing, line: &TermsLine, subtotal: u64, sats: u64) -> Value {
    let variant = VariantId(line.variant_id);
    let label = listing.commerce.as_ref().and_then(|c| {
        let found = c.variants.get(variant)?;
        Some(c.variants.describe(found).into_iter().map(|(_, value)| value).collect::<Vec<_>>().join(" / "))
    });
    json!({
        "listingHash": hex(&line.listing_hash.0),
        "listingId": listing.id().map(|id| id.to_hex()),
        "variant": hex(&line.variant_id),
        "variantLabel": label,
        "title": listing.title,
        "kind": listing.kind.as_str(),
        "quantity": line.quantity,
        "subtotal": subtotal,
        "priceSats": sats,
    })
}

impl Node {
    pub(crate) fn stock(&self) -> std::sync::MutexGuard<'_, StockLedger> {
        self.stock.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn order_terms(&self) -> std::sync::MutexGuard<'_, TermsLedger> {
        self.order_terms.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Price a version 3 order and hold its units under `id`. The inner
    /// error is a reason to decline; the outer one means try again later.
    pub(crate) fn price_terms_order(&self, id: &str, order: &Order, sender: &[u8; 32]) -> Result<Result<Priced, String>, MailError> {
        if order.options.get("settlement").map(String::as_str) != Some("direct") {
            return Ok(Err("This store takes direct payment only.".into()));
        }
        let terms = &order.terms;
        let listings: Option<Vec<Listing>> = {
            let own = self.own_listings();
            terms.lines.iter().map(|line| own.by_hash(&hex(&line.listing_hash.0)).cloned()).collect()
        };
        let Some(listings) = listings else {
            return Ok(Err("A listing in this order is no longer for sale.".into()));
        };
        if listings.iter().any(|listing| listing.id().is_none()) {
            return Ok(Err("Order this listing without options.".into()));
        }
        let pairs: Vec<(&Listing, &TermsLine)> = listings.iter().zip(&terms.lines).collect();
        let coupons = self.coupons().all().to_vec();
        let now = crate::escrow_outbox::now_secs();
        let checked = match self.order_terms().check(&pairs, terms, sender, &coupons, crate::own_store::today(), now) {
            Ok(checked) => checked,
            Err(reason) => return Ok(Err(reason)),
        };
        let quote = &checked.quote;
        let rate = self.btc_price(&quote.currency).ok_or_else(|| MailError::Retry(format!("no {} rate to price an order", quote.currency)))?;
        let sats = |minor: u64| if minor == 0 { Some(0) } else { crate::fiat::sats_for(minor, &quote.currency, rate) };
        let (Some(items), Some(shipping)) = (sats(quote.items - quote.coupon_off), sats(quote.shipping)) else {
            return Ok(Err("The order total is too small or too large to pay.".into()));
        };
        if let Err(reason) = self.hold_units(id, &pairs, now + PAYMENT_WINDOW) {
            return Ok(Err(reason));
        }
        let lines = pairs
            .iter()
            .zip(&quote.lines)
            .map(|((listing, line), priced)| line_value(listing, line, priced.subtotal, sats(priced.subtotal).unwrap_or(0)))
            .collect();
        let quote_value = json!({
            "currency": quote.currency,
            "items": quote.items,
            "couponOff": quote.coupon_off,
            "coupon": checked.coupon,
            "shipping": quote.shipping,
            "total": quote.total,
            "rate": rate,
            "reviewKey": (terms.review_key != [0; 32]).then(|| hex(&terms.review_key)),
        });
        Ok(Ok(Priced {
            lines,
            items,
            shipping,
            digital: listings.iter().any(|listing| listing.kind == ListingKind::Digital),
            physical: listings.iter().any(|listing| listing.kind == ListingKind::Physical),
            quote: quote_value,
            checked: Some(checked),
        }))
    }

    /// Hold every line's units, or none of them.
    fn hold_units(&self, id: &str, pairs: &[(&Listing, &TermsLine)], expires_at: u64) -> Result<(), String> {
        let mut stock = self.stock();
        for (listing, line) in pairs {
            let variant = VariantId(line.variant_id);
            if unlimited(listing, variant) {
                continue;
            }
            let key: Key = (listing.id().expect("checked above"), variant);
            match stock.reserve(id, key, line.quantity, signed_stock(listing, variant), expires_at) {
                Ok(()) => {}
                Err(refused) => {
                    if let Err(err) = stock.release(id) {
                        eprintln!("ob-node: sale {id}: {err}");
                    }
                    return Err(match refused {
                        Refused::Short { available: 0 } => format!("{} sold out.", listing.title),
                        Refused::Short { available } => format!("Only {available} of {} are left.", listing.title),
                        Refused::Failed(err) => {
                            eprintln!("ob-node: sale {id}: {err}");
                            "The store could not hold these items. Try again.".into()
                        }
                    });
                }
            }
        }
        Ok(())
    }

    /// The sale for a version 3 order was kept: count its coupon and spend its offers.
    pub(crate) fn record_terms(&self, id: &str, checked: &Checked) {
        if let Err(err) = self.order_terms().record(checked) {
            eprintln!("ob-node: sale {id}: {err}");
        }
    }

    /// Opening the sale failed after its units were held.
    pub(crate) fn release_units(&self, id: &str) {
        if let Err(err) = self.stock().release(id) {
            eprintln!("ob-node: sale {id}: {err}");
        }
    }
}
