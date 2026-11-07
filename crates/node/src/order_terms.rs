//! The seller's check of a version 3 order letter's terms: which variants,
//! how the package ships, and any coupon or accepted offers.
//!
//! The price comes from [`openbazaar_protocol::pricing::quote`], the same
//! rules the buyer's client ran. What only this node can know is checked
//! here too: how many times a coupon has been used, and whether an offer
//! has already been ordered with. [`TermsLedger::check`] does not change
//! anything; [`TermsLedger::record`] runs once the units are reserved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openbazaar_protocol::pricing::{quote, At, Delivery, Line, Quote};
use openbazaar_protocol::{Coupon, Listing, ListingKind, SignedCoupon, SignedOffer, VariantId};
use serde_json::{json, Value};
use tormail::{OrderTerms, TermsLine, NO_SERVICE};

use crate::escrow_mail::{hex, unhex};
use crate::vault::Vault;
use crate::Error;

const FILE: &str = "order-terms.obv";

/// A checked order line, ready to reserve and record.
#[derive(Debug)]
pub(crate) struct Checked {
    pub quote: Quote,
    pub coupon: Option<String>,
    pub offers: Vec<[u8; 16]>,
}

pub(crate) struct TermsLedger {
    path: PathBuf,
    vault: Vault,
    coupon_uses: BTreeMap<String, u32>,
    used_offers: BTreeSet<[u8; 16]>,
}

fn delivery(listing: &Listing, service: u8) -> Delivery {
    match (listing.kind, service) {
        (ListingKind::Digital, _) => Delivery::Digital,
        (ListingKind::Physical, NO_SERVICE) => Delivery::Pickup,
        (ListingKind::Physical, index) => Delivery::Service(index),
    }
}

impl TermsLedger {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut ledger = Self { path, vault, coupon_uses: BTreeMap::new(), used_offers: BTreeSet::new() };
        if let Some(bytes) = ledger.vault.read_sealed(&ledger.path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            for (code, uses) in value["couponUses"].as_object().into_iter().flatten() {
                ledger.coupon_uses.insert(code.clone(), uses.as_u64().unwrap_or(0) as u32);
            }
            for id in value["usedOffers"].as_array().into_iter().flatten() {
                if let Some(id) = id.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| bytes.try_into().ok()) {
                    ledger.used_offers.insert(id);
                }
            }
        }
        Ok(ledger)
    }

    fn save(&self) -> Result<(), String> {
        let used: Vec<String> = self.used_offers.iter().map(|id| hex(id)).collect();
        let bytes = serde_json::to_vec(&json!({ "v": 1, "couponUses": self.coupon_uses, "usedOffers": used })).map_err(|e| e.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|e| e.to_string())
    }

    pub(crate) fn coupon_uses(&self, code: &str) -> u32 {
        self.coupon_uses.get(code).copied().unwrap_or(0)
    }

    /// Check one order letter's lines, each with the listing its hash names.
    /// `sender` is the letter's conversation key, which an offer must be
    /// bound to. `coupons` are the coupons this store has signed and not
    /// withdrawn; the order names one by its code.
    pub(crate) fn check(
        &self,
        lines: &[(&Listing, &TermsLine)],
        terms: &OrderTerms,
        sender: &[u8; 32],
        coupons: &[SignedCoupon],
        today: u32,
        now: u64,
    ) -> Result<Checked, String> {
        let mut seen = BTreeSet::new();
        let mut per_listing: BTreeMap<String, u32> = BTreeMap::new();
        for (listing, line) in lines {
            let key = listing.content_hash().map_err(|err| err.to_string())?.to_hex();
            if !seen.insert((key.clone(), line.variant_id)) {
                return Err("Each variant may appear once in an order.".into());
            }
            let total = per_listing.entry(key).or_insert(0);
            *total += line.quantity;
            let max = listing.commerce.as_ref().map_or(0, |c| c.purchase.max_per_order);
            if max != 0 && *total > u32::from(max) {
                return Err("That is more than the seller sells in one order.".into());
            }
        }
        let mut offers = Vec::new();
        for (_, line) in lines {
            if line.offer.is_empty() {
                offers.push(None);
                continue;
            }
            let offer = SignedOffer::decode(&line.offer).map_err(|_| "The offer in the order could not be read.")?;
            if self.used_offers.contains(&offer.terms.offer_id) || offers.iter().flatten().any(|o: &SignedOffer| o.terms.offer_id == offer.terms.offer_id) {
                return Err("That offer has already been used.".into());
            }
            offers.push(Some(offer));
        }
        let coupon = match terms.coupon.as_slice() {
            [] => None,
            bytes => {
                let code = Coupon::normalize(std::str::from_utf8(bytes).unwrap_or(""));
                let found = coupons.iter().find(|c| c.terms.code == code).ok_or("This store does not have that coupon code.")?;
                let uses = self.coupon_uses.get(&found.terms.code).copied().unwrap_or(0);
                if found.terms.max_uses != 0 && uses >= found.terms.max_uses {
                    return Err("That coupon has been used up.".into());
                }
                Some(found)
            }
        };
        let priced: Vec<Line> = lines
            .iter()
            .zip(&offers)
            .map(|((listing, line), offer)| Line {
                listing,
                variant: VariantId(line.variant_id),
                quantity: line.quantity,
                delivery: delivery(listing, terms.service),
                offer: offer.as_ref(),
            })
            .collect();
        let at = At { today, now, buyer_key: Some(sender), country: None };
        let quote = quote(&priced, coupon, at).map_err(|err| crate::own_listings::capitalize(err.as_str()))?;
        let offers = offers.into_iter().flatten().map(|o| o.terms.offer_id).collect();
        Ok(Checked { quote, coupon: coupon.map(|c| c.terms.code.clone()), offers })
    }

    /// Count the coupon use and spend the offers of a checked order.
    pub(crate) fn record(&mut self, checked: &Checked) -> Result<(), String> {
        if let Some(code) = &checked.coupon {
            *self.coupon_uses.entry(code.clone()).or_insert(0) += 1;
        }
        self.used_offers.extend(checked.offers.iter().copied());
        if checked.coupon.is_some() || !checked.offers.is_empty() {
            self.save()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use openbazaar_protocol::{
        Availability, Commerce, ContentLabel, Discount, FiatPrice, ListingDetails, ListingId, Offer, StoreId,
    };

    use super::*;

    fn secret() -> bitcoin::secp256k1::SecretKey {
        bitcoin::secp256k1::SecretKey::from_slice(&[5; 32]).unwrap()
    }

    fn store() -> StoreId {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        StoreId::from_bytes(bitcoin::secp256k1::Keypair::from_secret_key(&secp, &secret()).x_only_public_key().0.serialize())
    }

    fn listing() -> Listing {
        let draft = json!({
            "axes": [{ "name": "Size", "values": ["S", "M"] }],
            "variants": [{ "options": ["S"], "stock": 2 }, { "options": ["M"], "stock": 1, "price": 3000 }],
            "maxPerOrder": 2,
            "availability": { "kind": "in-stock" },
            "shipping": { "services": [{ "name": "Post", "price": 500, "extraItem": 100 }], "pickup": "Oregon" }
        });
        let commerce: Commerce = crate::commerce_draft::parse(&draft, None).unwrap();
        assert_eq!(commerce.purchase.availability, Availability::InStock);
        let base = Listing::sign(
            store(), "Tee", "Shirts", 1, "Oregon", "A tee.", "Kiln", 3, 20261107, false, ListingKind::Physical, "", 0, 0,
            ContentLabel::GENERAL, &secret(),
        )
        .unwrap();
        Listing {
            details: Some(ListingDetails {
                id: ListingId([9; 16]),
                images: vec![],
                price: Some(FiatPrice { currency: "USD".into(), amount: 2_000, shipping: 0 }),
            }),
            commerce: Some(commerce),
            ..base
        }
        .resign(&secret())
        .unwrap()
    }

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ledger() -> (Dir, TermsLedger) {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ob-terms-{nanos}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = TermsLedger::open(&dir, Vault::from_key(&[3; 32])).unwrap();
        (Dir(dir), ledger)
    }

    fn line(listing: &Listing, variant: usize, quantity: u32, offer: Vec<u8>) -> TermsLine {
        let id = listing.commerce.as_ref().unwrap().variants.variants[variant].id;
        let hash = tormail::ListingHash(*listing.content_hash().unwrap().as_bytes());
        TermsLine { listing_hash: hash, variant_id: id.0, quantity, offer }
    }

    fn terms(lines: Vec<TermsLine>) -> OrderTerms {
        OrderTerms { lines, service: 0, review_key: [1; 32], ..OrderTerms::default() }
    }

    fn check(ledger: &TermsLedger, listing: &Listing, terms: &OrderTerms, sender: u8, coupons: &[SignedCoupon]) -> Result<Checked, String> {
        let lines: Vec<(&Listing, &TermsLine)> = terms.lines.iter().map(|line| (listing, line)).collect();
        ledger.check(&lines, terms, &[sender; 32], coupons, 20261107, 0)
    }

    #[test]
    fn a_variant_order_is_priced_from_the_signed_listing() {
        let (_dir, ledger) = ledger();
        let listing = listing();
        let checked = check(&ledger, &listing, &terms(vec![line(&listing, 1, 2, vec![])]), 2, &[]).unwrap();
        assert_eq!((checked.quote.items, checked.quote.shipping), (6_000, 600));
        let pickup = OrderTerms { service: NO_SERVICE, ..terms(vec![line(&listing, 0, 1, vec![])]) };
        assert_eq!(check(&ledger, &listing, &pickup, 2, &[]).unwrap().quote.total, 2_000);
        let err = check(&ledger, &listing, &terms(vec![line(&listing, 0, 3, vec![])]), 2, &[]).unwrap_err();
        assert_eq!(err, "That is more than the seller sells in one order.");
    }

    #[test]
    fn one_cart_holds_two_variants_of_a_listing_within_its_limit() {
        let (_dir, ledger) = ledger();
        let listing = listing();
        let both = terms(vec![line(&listing, 0, 1, vec![]), line(&listing, 1, 1, vec![])]);
        let checked = check(&ledger, &listing, &both, 2, &[]).unwrap();
        assert_eq!((checked.quote.items, checked.quote.shipping), (5_000, 600));
        let split = terms(vec![line(&listing, 0, 2, vec![]), line(&listing, 1, 1, vec![])]);
        assert_eq!(check(&ledger, &listing, &split, 2, &[]).unwrap_err(), "That is more than the seller sells in one order.");
        let twice = terms(vec![line(&listing, 0, 1, vec![]), line(&listing, 0, 1, vec![])]);
        assert_eq!(check(&ledger, &listing, &twice, 2, &[]).unwrap_err(), "Each variant may appear once in an order.");
    }

    #[test]
    fn coupons_run_out_and_offers_are_single_use() {
        let (dir, mut ledger) = ledger();
        let listing = listing();
        let coupon = SignedCoupon::sign(
            Coupon {
                store_id: store(), code: "ONCE".into(), discount: Discount::Percent(1_000), currency: "USD".into(),
                min_subtotal: 0, listings: vec![], starts_on: 20261101, ends_on: 20261130, max_uses: 1,
            },
            &secret(),
        )
        .unwrap();
        let with_coupon = OrderTerms { coupon: b" once ".to_vec(), ..terms(vec![line(&listing, 0, 1, vec![])]) };
        let checked = check(&ledger, &listing, &with_coupon, 2, std::slice::from_ref(&coupon)).unwrap();
        assert_eq!(checked.quote.coupon_off, 200);
        assert_eq!(check(&ledger, &listing, &with_coupon, 2, &[]).unwrap_err(), "This store does not have that coupon code.");
        ledger.record(&checked).unwrap();
        let ledger = TermsLedger::open(&dir.0, Vault::from_key(&[3; 32])).unwrap();
        assert_eq!(check(&ledger, &listing, &with_coupon, 2, &[coupon]).unwrap_err(), "That coupon has been used up.");

        let mut ledger = ledger;
        let variant = listing.commerce.as_ref().unwrap().variants.variants[0].id;
        let offer = SignedOffer::sign(
            Offer {
                store_id: store(), offer_id: [4; 16], listing_id: ListingId([9; 16]), variant_id: variant, quantity: 1,
                amount: 1_500, currency: "USD".into(), buyer_key: [2; 32], expires_at: 1_000, counter: false,
            },
            &secret(),
        )
        .unwrap();
        let with_offer = terms(vec![line(&listing, 0, 1, offer.encode().unwrap())]);
        assert_eq!(
            check(&ledger, &listing, &with_offer, 8, &[]).unwrap_err(),
            "The offer is not for this line or not from this seller.",
            "another conversation key cannot use it"
        );
        let checked = check(&ledger, &listing, &with_offer, 2, &[]).unwrap();
        assert_eq!(checked.quote.items, 1_500);
        ledger.record(&checked).unwrap();
        assert_eq!(check(&ledger, &listing, &with_offer, 2, &[]).unwrap_err(), "That offer has already been used.");
    }
}
