//! Version 7 listings and the coupon, offer, receipt and review records.

use bitcoin::secp256k1::SecretKey;

use crate::sign;
use crate::{
    Availability, Axis, Bid, Commerce, Condition, ContentLabel, Coupon, CountryCode, Discount, Error, FiatPrice,
    Listing, ListingDetails, ListingId, ListingKind, Offer, Pricing, Purchase, Receipt, Reply, Returns, Review, Sale,
    Service, Shipping, SignedCoupon, SignedOffer, SignedReceipt, SignedReply, SignedReview, StoreId, Tier, Variant, VariantId,
    Variants,
};

pub(crate) fn store_secret() -> SecretKey {
    sign::secret_from_label("commerce-test-store")
}

pub(crate) fn store_id() -> StoreId {
    StoreId::from_bytes(sign::xonly(&store_secret()))
}

pub(crate) fn variant(id: u8, choice: &[u8], price: u64, stock: u32) -> Variant {
    Variant { id: VariantId([id; 8]), choice: choice.to_vec(), sku: format!("TEE-{id}"), price, stock, image: 0, weight_g: 0 }
}

pub(crate) fn tee_commerce() -> Commerce {
    Commerce {
        condition: Condition::New,
        condition_note: String::new(),
        taxonomy: 3,
        specifics: vec![("brand".into(), "Marlow".into()), ("material".into(), "Cotton".into())],
        variants: Variants {
            axes: vec![
                Axis { name: "Size".into(), values: vec!["S".into(), "M".into(), "L".into()] },
                Axis { name: "Color".into(), values: vec!["Navy".into(), "Sand".into()] },
            ],
            variants: vec![
                variant(1, &[0, 0], 0, 4),
                variant(2, &[1, 0], 0, 0),
                variant(3, &[2, 0], 2_600, 2),
                variant(4, &[1, 1], 0, 6),
            ],
        },
        pricing: Pricing { compare_at: 3_200, sale: None, tiers: vec![Tier { min_qty: 3, off_bps: 1_000 }], accepts_offers: true },
        purchase: Purchase { max_per_order: 5, lot_size: 1, availability: Availability::InStock },
        shipping: Shipping {
            weight_g: 220,
            dims_mm: [300, 200, 30],
            handling_days: 2,
            free_over: 10_000,
            services: vec![
                Service { name: "Standard".into(), price: 500, extra_item: 150, days_min: 3, days_max: 6, regions: vec![] },
                Service {
                    name: "Express".into(),
                    price: 1_500,
                    extra_item: 300,
                    days_min: 1,
                    days_max: 2,
                    regions: vec![CountryCode::parse("CA").unwrap(), CountryCode::parse("US").unwrap()],
                },
            ],
            pickup: Some("Oregon".into()),
        },
        returns: Returns::Days { days: 30, seller_pays: false },
    }
}

/// A signed v7 listing; `stock` is set to the variants' total.
pub(crate) fn v7(id: u8, price: u64, kind: ListingKind, commerce: Commerce) -> Result<Listing, Error> {
    let stock = if commerce.variants.is_empty() { 5 } else { commerce.variants.total_stock() as u32 };
    let base = Listing::sign(
        store_id(),
        "Pocket tee",
        "Shirts",
        30_000,
        if kind == ListingKind::Physical { "Oregon" } else { "" },
        "A heavyweight cotton tee.",
        "Marlow Goods",
        stock,
        20261107,
        false,
        kind,
        "",
        0,
        0,
        ContentLabel::GENERAL,
        &store_secret(),
    )?;
    Listing {
        details: Some(ListingDetails {
            id: ListingId([id; 16]),
            images: vec![[1; 32], [2; 32]],
            price: Some(FiatPrice { currency: "USD".into(), amount: price, shipping: 700 }),
        }),
        commerce: Some(commerce),
        ..base
    }
    .resign(&store_secret())
}

fn tee() -> Listing {
    v7(9, 2_400, ListingKind::Physical, tee_commerce()).unwrap()
}

fn with(change: impl FnOnce(&mut Commerce)) -> Result<Listing, Error> {
    let mut commerce = tee_commerce();
    change(&mut commerce);
    v7(9, 2_400, ListingKind::Physical, commerce)
}

#[test]
fn v7_round_trips_inside_the_signature() {
    let listing = tee();
    assert_eq!(listing.version(), 7);
    assert_eq!(listing.stock, 12);
    let bytes = listing.encode().unwrap();
    assert_eq!(bytes[4], 7);
    let decoded = Listing::decode(&bytes).unwrap();
    assert_eq!(decoded, listing);
    decoded.verify().unwrap();
    assert_eq!(decoded.encode().unwrap(), bytes);
    let commerce = decoded.commerce.unwrap();
    let large = commerce.variants.get(VariantId([3; 8])).unwrap();
    assert_eq!(commerce.variants.describe(large), vec![("Size".into(), "L".into()), ("Color".into(), "Navy".into())]);
}

#[test]
fn every_commerce_byte_is_signed() {
    let bytes = tee().encode().unwrap();
    let start = bytes.len() - 64 - 200;
    for at in start..bytes.len() - 64 {
        let mut changed = bytes.clone();
        changed[at] ^= 0x01;
        if let Ok(listing) = Listing::decode(&changed) {
            assert_eq!(listing.verify(), Err(Error::BadSignature), "byte {at}");
        }
    }
}

#[test]
fn stripping_commerce_fails_verify() {
    let listing = tee();
    let downgraded = Listing { commerce: None, ..listing };
    assert_eq!(downgraded.verify(), Err(Error::BadSignature));
}

#[test]
fn stock_is_the_sum_of_variant_stock() {
    let mut listing = tee();
    listing.stock = 99;
    assert_eq!(listing.resign(&store_secret()).err(), Some(Error::BadListing("stock is the sum of the variants' stock")));
}

#[test]
fn v7_needs_a_fiat_price() {
    let mut listing = tee();
    listing.details.as_mut().unwrap().price = None;
    assert_eq!(listing.resign(&store_secret()).err(), Some(Error::BadListing("a version 7 listing has a fiat price")));
}

#[test]
fn variant_rules() {
    let dup_choice = with(|c| c.variants.variants[1].choice = vec![0, 0]);
    assert_eq!(dup_choice.err(), Some(Error::BadListing("two variants pick the same options")));
    let dup_id = with(|c| c.variants.variants[1].id = VariantId([1; 8]));
    assert_eq!(dup_id.err(), Some(Error::BadListing("a variant id is used twice")));
    let out_of_range = with(|c| c.variants.variants[0].choice = vec![3, 0]);
    assert_eq!(out_of_range.err(), Some(Error::BadListing("a variant picks a value that is not offered")));
    let bad_image = with(|c| c.variants.variants[0].image = 3);
    assert_eq!(bad_image.err(), Some(Error::BadListing("a variant names an image the listing does not have")));
    let dup_value = with(|c| c.variants.axes[0].values[1] = "s".into());
    assert_eq!(dup_value.err(), Some(Error::BadListing("an option value is used twice")));
    let add_axes = |names: &[&str]| {
        with(|c| {
            for name in names {
                c.variants.axes.push(Axis { name: (*name).into(), values: vec!["x".into()] });
                for v in &mut c.variants.variants {
                    v.choice.push(0);
                }
            }
        })
    };
    assert!(add_axes(&["Material"]).is_ok());
    assert_eq!(add_axes(&["Material", "Fit"]).err(), Some(Error::BadListing("a listing has at most 3 option axes")));
    let no_variants = v7(9, 2_400, ListingKind::Physical, Commerce { variants: Variants::default(), ..tee_commerce() }).unwrap();
    assert_eq!(no_variants.stock, 5);
}

#[test]
fn pricing_rules() {
    let low_compare = with(|c| c.pricing.compare_at = 2_600);
    assert!(matches!(low_compare, Err(Error::BadListing(_))), "must beat the dearest variant");
    let backwards_sale = with(|c| c.pricing.sale = Some(Sale { off_bps: 1_000, starts_on: 20261110, ends_on: 20261101 }));
    assert_eq!(backwards_sale.err(), Some(Error::BadListing("a sale ends before it starts")));
    let flat_tiers = with(|c| c.pricing.tiers.push(Tier { min_qty: 5, off_bps: 1_000 }));
    assert_eq!(flat_tiers.err(), Some(Error::BadListing("volume tiers rise in quantity and discount")));
    let zero_lot = with(|c| c.purchase.lot_size = 0);
    assert!(zero_lot.is_err());
}

#[test]
fn digital_listings_have_no_shipping() {
    let digital = v7(9, 2_400, ListingKind::Digital, tee_commerce());
    assert_eq!(digital.err(), Some(Error::BadListing("a digital listing has no shipping")));
    let ok = v7(9, 2_400, ListingKind::Digital, Commerce { shipping: Shipping::default(), ..tee_commerce() });
    assert!(ok.is_ok());
}

#[test]
fn pickup_is_a_coarse_region_only() {
    for fine in ["Portland, Oregon", "SE Hawthorne 97214", "12 Cedar Street"] {
        let listing = with(|c| c.shipping.pickup = Some(fine.into()));
        assert!(matches!(listing, Err(Error::PrivateText { field: "pickup area", .. })), "{fine}");
    }
}

#[test]
fn specifics_are_sorted_checked_and_private() {
    let unsorted = with(|c| c.specifics.reverse());
    assert_eq!(unsorted.err(), Some(Error::BadListing("item specifics are sorted by key, each key once")));
    let isbn = with(|c| c.specifics.insert(1, ("isbn".into(), "9780306406157".into())));
    assert!(isbn.is_ok(), "a product code is not a phone number");
    let wrong = with(|c| c.specifics.insert(1, ("isbn".into(), "9780306406158".into())));
    assert_eq!(wrong.err(), Some(Error::BadListing("a product code's check digit is wrong")));
    let leaky = with(|c| c.specifics[0].1 = "call 503-555-0142".into());
    assert!(matches!(leaky, Err(Error::PrivateText { field: "item specific", .. })));
    let leaky_note = with(|c| c.condition_note = "see me at 221B Baker Street".into());
    assert!(matches!(leaky_note, Err(Error::PrivateText { field: "condition note", .. })));
}

#[test]
fn older_versions_still_decode() {
    let listing = tee();
    let v6 = Listing { commerce: None, ..listing }.resign(&store_secret()).unwrap();
    assert_eq!(v6.version(), 6);
    let decoded = Listing::decode(&v6.encode().unwrap()).unwrap();
    decoded.verify().unwrap();
    assert_eq!(decoded.commerce, None);
}

pub(crate) fn coupon(discount: Discount) -> Coupon {
    Coupon {
        store_id: store_id(),
        code: "FALL-10".into(),
        discount,
        currency: "USD".into(),
        min_subtotal: 0,
        listings: vec![],
        starts_on: 20261101,
        ends_on: 20261130,
        max_uses: 100,
    }
}

#[test]
fn coupons_are_signed_by_the_store() {
    let signed = SignedCoupon::sign(coupon(Discount::Percent(1_000)), &store_secret()).unwrap();
    let decoded = SignedCoupon::decode(&signed.encode().unwrap()).unwrap();
    decoded.verify().unwrap();
    assert_eq!(decoded, signed);
    let mut richer = decoded.clone();
    richer.terms.discount = Discount::Percent(5_000);
    assert_eq!(richer.verify(), Err(Error::BadSignature));
    let forged = SignedCoupon::sign(coupon(Discount::Fixed(500)), &sign::secret_from_label("not-the-store"));
    assert_eq!(forged.err(), Some(Error::BadKey));
    let lower = SignedCoupon::sign(Coupon { code: "fall".into(), ..coupon(Discount::Fixed(500)) }, &store_secret());
    assert!(matches!(lower, Err(Error::BadTerms(_))));
    assert_eq!(Coupon::normalize(" fall-10 "), "FALL-10");
}

pub(crate) fn offer(buyer: [u8; 32], amount: u64, quantity: u16) -> SignedOffer {
    SignedOffer::sign(
        Offer {
            store_id: store_id(),
            offer_id: [5; 16],
            listing_id: ListingId([9; 16]),
            variant_id: VariantId([4; 8]),
            quantity,
            amount,
            currency: "USD".into(),
            buyer_key: buyer,
            expires_at: 1_800_000_000,
            counter: true,
        },
        &store_secret(),
    )
    .unwrap()
}

#[test]
fn offers_and_bids_round_trip() {
    let signed = offer([8; 32], 2_000, 1);
    let decoded = SignedOffer::decode(&signed.encode().unwrap()).unwrap();
    decoded.verify().unwrap();
    let mut rebound = decoded.clone();
    rebound.terms.buyer_key = [7; 32];
    assert_eq!(rebound.verify(), Err(Error::BadSignature));
    assert!(decoded.terms.live_at(1_799_999_999));
    assert!(!decoded.terms.live_at(1_800_000_000));

    let bid = Bid { listing_id: ListingId([9; 16]), variant_id: VariantId::NONE, quantity: 2, amount: 1_800, currency: "USD".into() };
    assert_eq!(Bid::decode(&bid.encode().unwrap()).unwrap(), bid);
    assert!(Bid { quantity: 0, ..bid }.encode().is_err());
}

#[test]
fn reviews_need_a_receipt_from_the_store() {
    let review_secret = sign::secret_from_label("order-7-review");
    let receipt = SignedReceipt::sign(
        Receipt { store_id: store_id(), listing_id: ListingId([9; 16]), review_key: sign::xonly(&review_secret), paid_on: 20261102 },
        &store_secret(),
    )
    .unwrap();
    let review = SignedReview::sign(
        Review { receipt: receipt.clone(), rating: 5, text: "Soft and well made.".into(), written_on: 20261109 },
        &review_secret,
    )
    .unwrap();
    let decoded = SignedReview::decode(&review.encode().unwrap()).unwrap();
    decoded.verify_purchase().unwrap();
    assert_eq!(decoded.listing_id(), ListingId([9; 16]));

    let mut softened = decoded.clone();
    softened.terms.rating = 4;
    assert_eq!(softened.verify_purchase(), Err(Error::BadSignature));

    let self_receipt = SignedReceipt { signature: [1; 64], ..receipt.clone() };
    let fake = SignedReview::sign(Review { receipt: self_receipt, rating: 5, text: String::new(), written_on: 20261109 }, &review_secret).unwrap();
    assert!(fake.verify_purchase().is_err());

    let wrong_key = SignedReview::sign(
        Review { receipt: receipt.clone(), rating: 1, text: String::new(), written_on: 20261109 },
        &sign::secret_from_label("someone-else"),
    );
    assert_eq!(wrong_key.err(), Some(Error::BadKey));

    let early = SignedReview::sign(Review { receipt: receipt.clone(), rating: 3, text: String::new(), written_on: 20261101 }, &review_secret);
    assert!(matches!(early, Err(Error::BadTerms(_))));

    let doxxing = SignedReview::sign(Review { receipt, rating: 1, text: "Seller lives at 14 Elm Street".into(), written_on: 20261109 }, &review_secret);
    assert!(matches!(doxxing, Err(Error::PrivateText { field: "review", .. })));
}

#[test]
fn a_reply_answers_one_review_and_only_its_store_signs_it() {
    let review_secret = sign::secret_from_label("order-8-review");
    let receipt = SignedReceipt::sign(
        Receipt { store_id: store_id(), listing_id: ListingId([9; 16]), review_key: sign::xonly(&review_secret), paid_on: 20261101 },
        &store_secret(),
    )
    .unwrap();
    let review = SignedReview::sign(Review { receipt, rating: 2, text: "Arrived late.".into(), written_on: 20261110 }, &review_secret).unwrap();
    let reply = SignedReply::sign(
        Reply { store_id: store_id(), review: review.hash().unwrap(), text: "Sorry, the post office lost a week.".into(), written_on: 20261111 },
        &store_secret(),
    )
    .unwrap();
    let decoded = SignedReply::decode(&reply.encode().unwrap()).unwrap();
    assert!(decoded.answers(&review));

    let other = SignedReview::sign(Review { rating: 5, ..review.terms.clone() }, &review_secret).unwrap();
    assert!(!decoded.answers(&other), "a reply names one review's hash");
    let forged = Reply { store_id: store_id(), review: review.hash().unwrap(), text: "Thanks!".into(), written_on: 20261111 };
    assert_eq!(SignedReply::sign(forged, &review_secret).err(), Some(Error::BadKey));
}
