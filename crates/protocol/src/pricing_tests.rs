//! The package price rules, and the golden vectors the client checks itself against.
//!
//! `vectors/pricing.json` holds signed records (hex), the lines and the
//! expected quote or error. Regenerate it with `OB_UPDATE_VECTORS=1`.

use serde_json::{json, Value};

use crate::commerce_tests::{coupon, offer, store_secret, tee_commerce, v7};
use crate::pricing::{quote, At, Delivery, Line, Quote, QuoteError};
use crate::sign;
use crate::{
    Availability, Commerce, CountryCode, Discount, Listing, ListingId, ListingKind, Sale, Shipping, SignedCoupon,
    SignedOffer, VariantId, Variants,
};

const TODAY: u32 = 20261107;
const NOW: u64 = 1_790_000_000;

fn tee() -> Listing {
    v7(9, 2_400, ListingKind::Physical, tee_commerce()).unwrap()
}

/// A second listing from the same store: no variants, cheaper shipping.
fn mug() -> Listing {
    let mut commerce = Commerce { variants: Variants::default(), ..tee_commerce() };
    commerce.pricing.compare_at = 0;
    commerce.pricing.tiers.clear();
    commerce.shipping.services[0].price = 400;
    commerce.shipping.services[0].extra_item = 100;
    commerce.shipping.free_over = 0;
    v7(10, 1_800, ListingKind::Physical, commerce).unwrap()
}

fn ebook() -> Listing {
    let commerce = Commerce { variants: Variants::default(), shipping: Shipping::default(), ..tee_commerce() };
    v7(11, 900, ListingKind::Digital, commerce).unwrap()
}

fn line(listing: &Listing, variant: u8, quantity: u32, delivery: Delivery) -> Line<'_> {
    let variant = if variant == 0 { VariantId::NONE } else { VariantId([variant; 8]) };
    Line { listing, variant, quantity, delivery, offer: None }
}

fn at() -> At<'static> {
    At { today: TODAY, now: NOW, buyer_key: None, country: None }
}

fn signed_coupon(discount: Discount, change: impl FnOnce(&mut crate::Coupon)) -> SignedCoupon {
    let mut terms = coupon(discount);
    change(&mut terms);
    SignedCoupon::sign(terms, &store_secret()).unwrap()
}

#[test]
fn variant_price_tiers_and_shipping() {
    let tee = tee();
    let q = quote(&[line(&tee, 3, 1, Delivery::Service(0))], None, at()).unwrap();
    assert_eq!((q.items, q.shipping, q.total), (2_600, 500, 3_100), "variant price override");
    let q = quote(&[line(&tee, 4, 3, Delivery::Service(1))], None, at()).unwrap();
    assert_eq!(q.lines[0].unit, 2_400);
    assert_eq!(q.items, 6_480, "10% off at 3");
    assert_eq!(q.shipping, 1_500 + 2 * 300, "express plus two extra items");
}

#[test]
fn sale_runs_only_on_its_days() {
    let mut commerce = tee_commerce();
    commerce.pricing.sale = Some(Sale { off_bps: 2_500, starts_on: 20261101, ends_on: TODAY });
    let listing = v7(9, 2_400, ListingKind::Physical, commerce).unwrap();
    let q = quote(&[line(&listing, 1, 1, Delivery::Service(0))], None, at()).unwrap();
    assert_eq!(q.lines[0].unit, 1_800);
    let tomorrow = At { today: TODAY + 1, ..at() };
    assert_eq!(quote(&[line(&listing, 1, 1, Delivery::Service(0))], None, tomorrow).unwrap().lines[0].unit, 2_400);
}

#[test]
fn combined_shipping_and_the_free_threshold() {
    let (tee, mug) = (tee(), mug());
    let q = quote(&[line(&mug, 0, 2, Delivery::Service(0)), line(&tee, 1, 1, Delivery::Service(0))], None, at()).unwrap();
    assert_eq!(q.shipping, 500 + 100 * 2, "the tee's service is dearer and pays full price");
    let q = quote(&[line(&tee, 4, 4, Delivery::Service(0))], None, at()).unwrap();
    assert_eq!(q.items, 8_640);
    assert_eq!(q.shipping, 500 + 3 * 150, "below the 100.00 threshold");
    let q = quote(&[line(&tee, 4, 5, Delivery::Service(0))], None, at()).unwrap();
    assert_eq!((q.items, q.shipping), (10_800, 0));
}

#[test]
fn pickup_digital_and_delivery_mistakes() {
    let (tee, ebook) = (tee(), ebook());
    let q = quote(&[line(&tee, 1, 1, Delivery::Pickup), line(&ebook, 0, 1, Delivery::Digital)], None, at()).unwrap();
    assert_eq!((q.items, q.shipping), (3_300, 0));
    assert_eq!(quote(&[line(&ebook, 0, 1, Delivery::Service(0))], None, at()), Err(QuoteError::WrongDelivery));
    assert_eq!(quote(&[line(&tee, 1, 1, Delivery::Digital)], None, at()), Err(QuoteError::WrongDelivery));
    assert_eq!(quote(&[line(&tee, 1, 1, Delivery::Service(2))], None, at()), Err(QuoteError::NoSuchService));
    let de = CountryCode::parse("DE").unwrap();
    let abroad = At { country: Some(&de), ..at() };
    assert_eq!(quote(&[line(&tee, 1, 1, Delivery::Service(1))], None, abroad), Err(QuoteError::NotShippable));
    assert!(quote(&[line(&tee, 1, 1, Delivery::Service(0))], None, abroad).is_ok());
}

#[test]
fn limits_variants_and_availability() {
    let tee = tee();
    assert_eq!(quote(&[line(&tee, 1, 6, Delivery::Service(0))], None, at()), Err(QuoteError::OverLimit));
    assert_eq!(quote(&[line(&tee, 0, 1, Delivery::Service(0))], None, at()), Err(QuoteError::NoSuchVariant));
    assert_eq!(quote(&[line(&tee, 7, 1, Delivery::Service(0))], None, at()), Err(QuoteError::NoSuchVariant));
    let mut commerce = tee_commerce();
    commerce.purchase.availability = Availability::Away;
    let away = v7(9, 2_400, ListingKind::Physical, commerce).unwrap();
    assert_eq!(quote(&[line(&away, 1, 1, Delivery::Service(0))], None, at()), Err(QuoteError::Away));
}

#[test]
fn coupons_cover_their_scope_only() {
    let (tee, mug) = (tee(), mug());
    let lines = [line(&tee, 1, 1, Delivery::Service(0)), line(&mug, 0, 1, Delivery::Service(0))];
    let all = signed_coupon(Discount::Percent(1_000), |_| {});
    assert_eq!(quote(&lines, Some(&all), at()).unwrap().coupon_off, 420);
    let mug_only = signed_coupon(Discount::Fixed(5_000), |c| c.listings = vec![ListingId([10; 16])]);
    assert_eq!(quote(&lines, Some(&mug_only), at()).unwrap().coupon_off, 1_800, "never more than the lines it covers");
    let minimum = signed_coupon(Discount::Fixed(500), |c| c.min_subtotal = 5_000);
    assert_eq!(quote(&lines, Some(&minimum), at()), Err(QuoteError::CouponMinimum));
    let other = signed_coupon(Discount::Fixed(500), |c| c.listings = vec![ListingId([42; 16])]);
    assert_eq!(quote(&lines, Some(&other), at()), Err(QuoteError::CouponScope));
    let expired = signed_coupon(Discount::Fixed(500), |c| c.ends_on = TODAY - 1);
    assert_eq!(quote(&lines, Some(&expired), at()), Err(QuoteError::CouponDates));
    let mut forged = all.clone();
    forged.terms.discount = Discount::Percent(9_000);
    assert_eq!(quote(&lines, Some(&forged), at()), Err(QuoteError::BadCoupon));
}

#[test]
fn an_offer_sets_the_unit_price_for_its_buyer_only() {
    let tee = tee();
    let buyer = sign::xonly(&sign::secret_from_label("conversation-key"));
    let accepted = offer(buyer, 2_000, 2);
    fn with_offer<'a>(tee: &'a Listing, offer: &'a SignedOffer, quantity: u32) -> Line<'a> {
        Line { offer: Some(offer), ..line(tee, 4, quantity, Delivery::Service(0)) }
    }
    let with_offer = |offer, quantity| with_offer(&tee, offer, quantity);
    let mine = At { buyer_key: Some(&buyer), ..at() };
    let q = quote(&[with_offer(&accepted, 2)], None, mine).unwrap();
    assert_eq!((q.lines[0].unit, q.items, q.lines[0].offered), (2_000, 4_000, true));
    assert_eq!(quote(&[with_offer(&accepted, 3)], None, mine), Err(QuoteError::BadOffer), "quantity is part of the offer");
    let stranger = [3u8; 32];
    assert_eq!(quote(&[with_offer(&accepted, 2)], None, At { buyer_key: Some(&stranger), ..at() }), Err(QuoteError::BadOffer));
    assert_eq!(quote(&[with_offer(&accepted, 2)], None, At { now: 1_800_000_000, ..mine }), Err(QuoteError::OfferExpired));
    let coupon = signed_coupon(Discount::Percent(1_000), |_| {});
    assert_eq!(quote(&[with_offer(&accepted, 2)], Some(&coupon), mine), Err(QuoteError::CouponScope), "offers do not stack with coupons");
}

#[test]
fn one_store_and_one_currency_per_package() {
    let tee = tee();
    let other = {
        let secret = sign::secret_from_label("another-store");
        let mut listing = tee.clone();
        listing.store_id = crate::StoreId::from_bytes(sign::xonly(&secret));
        listing.resign(&secret).unwrap()
    };
    let lines = [line(&tee, 1, 1, Delivery::Service(0)), line(&other, 1, 1, Delivery::Service(0))];
    assert_eq!(quote(&lines, None, at()), Err(QuoteError::MixedStores));
    let mut euro = mug();
    euro.details.as_mut().unwrap().price.as_mut().unwrap().currency = "EUR".into();
    let euro = euro.resign(&store_secret()).unwrap();
    let lines = [line(&tee, 1, 1, Delivery::Service(0)), line(&euro, 0, 1, Delivery::Service(0))];
    assert_eq!(quote(&lines, None, at()), Err(QuoteError::MixedCurrencies));
}

fn delivery_json(delivery: Delivery) -> Value {
    match delivery {
        Delivery::Service(i) => json!({ "service": i }),
        Delivery::Pickup => json!("pickup"),
        Delivery::Digital => json!("digital"),
    }
}

fn vector(name: &str, lines: &[Line<'_>], coupon: Option<&SignedCoupon>, today: u32) -> Value {
    let result = quote(lines, coupon, At { today, ..at() });
    let mut records: Vec<String> = Vec::new();
    let lines_json: Vec<Value> = lines
        .iter()
        .map(|line| {
            let hex = crate::encode::to_hex(&line.listing.encode().unwrap());
            let index = records.iter().position(|r| *r == hex).unwrap_or_else(|| {
                records.push(hex);
                records.len() - 1
            });
            json!({
                "listing": index,
                "variant": line.variant.to_hex(),
                "quantity": line.quantity,
                "delivery": delivery_json(line.delivery),
            })
        })
        .collect();
    let expect = match result {
        Ok(Quote { lines, items, coupon_off, shipping, total, currency }) => json!({
            "currency": currency,
            "units": lines.iter().map(|l| l.unit).collect::<Vec<_>>(),
            "subtotals": lines.iter().map(|l| l.subtotal).collect::<Vec<_>>(),
            "items": items, "couponOff": coupon_off, "shipping": shipping, "total": total,
        }),
        Err(error) => json!({ "error": format!("{error:?}") }),
    };
    json!({
        "name": name,
        "today": today,
        "records": records,
        "lines": lines_json,
        "coupon": coupon.map(|c| crate::encode::to_hex(&c.encode().unwrap())),
        "expect": expect,
    })
}

#[test]
fn golden_vectors_match_the_file() {
    let (tee, mug, ebook) = (tee(), mug(), ebook());
    let mut sale = tee_commerce();
    sale.pricing.sale = Some(Sale { off_bps: 2_500, starts_on: 20261101, ends_on: TODAY });
    let sale = v7(9, 2_400, ListingKind::Physical, sale).unwrap();
    let ten = signed_coupon(Discount::Percent(1_000), |_| {});
    let mug_only = signed_coupon(Discount::Fixed(5_000), |c| c.listings = vec![ListingId([10; 16])]);
    let s0 = Delivery::Service(0);
    let vectors = vec![
        vector("variant price override", &[line(&tee, 3, 1, s0)], None, TODAY),
        vector("volume tier and express", &[line(&tee, 4, 3, Delivery::Service(1))], None, TODAY),
        vector("sale on its last day", &[line(&sale, 1, 2, s0)], None, TODAY),
        vector("sale over", &[line(&sale, 1, 2, s0)], None, TODAY + 1),
        vector("combined shipping", &[line(&mug, 0, 2, s0), line(&tee, 1, 1, s0)], None, TODAY),
        vector("free over threshold", &[line(&tee, 4, 5, s0)], None, TODAY),
        vector("pickup and digital", &[line(&tee, 1, 1, Delivery::Pickup), line(&ebook, 0, 1, Delivery::Digital)], None, TODAY),
        vector("percent coupon", &[line(&tee, 1, 1, s0), line(&mug, 0, 1, s0)], Some(&ten), TODAY),
        vector("scoped fixed coupon", &[line(&tee, 1, 1, s0), line(&mug, 0, 1, s0)], Some(&mug_only), TODAY),
        vector("coupon expired", &[line(&mug, 0, 1, s0)], Some(&ten), 20261201),
        vector("over the per-order limit", &[line(&tee, 1, 6, s0)], None, TODAY),
        vector("variant required", &[line(&tee, 0, 1, s0)], None, TODAY),
    ];
    let text = serde_json::to_string_pretty(&json!({ "vectors": vectors })).unwrap() + "\n";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/vectors/pricing.json");
    if std::env::var_os("OB_UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/vectors")).unwrap();
        std::fs::write(path, &text).unwrap();
    }
    let on_disk = std::fs::read_to_string(path).expect("vectors/pricing.json; run with OB_UPDATE_VECTORS=1");
    assert_eq!(on_disk, text, "pricing vectors changed; regenerate with OB_UPDATE_VECTORS=1 and update the client");
}
