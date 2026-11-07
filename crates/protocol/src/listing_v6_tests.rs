//! Version 6 listings: stable id, images, and a fiat price inside the signed body.

use crate::sign;
use crate::{
    ContentLabel, Error, FiatPrice, Listing, ListingDetails, ListingId, ListingKind, StoreId, MAX_IMAGES,
};

fn secret() -> bitcoin::secp256k1::SecretKey {
    sign::secret_from_label("listing-v6-test")
}

fn v5() -> Listing {
    let secret = secret();
    Listing::sign(
        StoreId::from_bytes(sign::xonly(&secret)),
        "Hand-thrown bowl",
        "Home",
        25_000,
        "Vermont",
        "A small stoneware bowl.",
        "Kiln Corner",
        3,
        20261107,
        false,
        ListingKind::Physical,
        "",
        1_500,
        0,
        ContentLabel::GENERAL,
        &secret,
    )
    .unwrap()
}

fn v6() -> Listing {
    Listing {
        details: Some(ListingDetails {
            id: ListingId([7; 16]),
            images: vec![[1; 32], [2; 32]],
            price: Some(FiatPrice { currency: "USD".into(), amount: 2_400, shipping: 500 }),
        }),
        ..v5()
    }
    .resign(&secret())
    .unwrap()
}

#[test]
fn details_round_trip_inside_the_signature() {
    let listing = v6();
    assert_eq!(listing.version(), 6);
    let bytes = listing.encode().unwrap();
    assert_eq!(bytes[4], 6);
    let decoded = Listing::decode(&bytes).unwrap();
    assert_eq!(decoded, listing);
    decoded.verify().unwrap();
    assert_eq!(decoded.id(), Some(ListingId([7; 16])));
    assert_eq!(v5().version(), 5);
    assert_eq!(v5().id(), None);
}

#[test]
fn an_edit_keeps_the_id_and_moves_the_address() {
    let first = v6();
    let mut edited = first.clone();
    edited.title = "Hand-thrown bowl, large".into();
    let edited = edited.resign(&secret()).unwrap();
    assert_eq!(edited.id(), first.id());
    assert_ne!(edited.content_hash().unwrap(), first.content_hash().unwrap());
}

#[test]
fn changing_any_detail_fails_verify() {
    let listing = v6();
    let tamper: [&dyn Fn(&mut ListingDetails); 4] = [
        &|d| d.id.0[0] ^= 1,
        &|d| d.images.reverse(),
        &|d| d.price.as_mut().unwrap().amount += 1,
        &|d| d.price = None,
    ];
    for change in tamper {
        let mut forged = listing.clone();
        change(forged.details.as_mut().unwrap());
        assert_eq!(forged.verify(), Err(Error::BadSignature));
    }
    let mut stripped = listing.clone();
    stripped.details = None;
    assert_eq!(stripped.verify(), Err(Error::BadSignature));
}

#[test]
fn bad_details_are_refused_when_signing_and_decoding() {
    let with = |details: ListingDetails| Listing { details: Some(details), ..v5() }.resign(&secret());
    let base = ListingDetails { id: ListingId([1; 16]), images: Vec::new(), price: None };
    assert!(with(base.clone()).is_ok());
    let many = ListingDetails { images: (0..=MAX_IMAGES as u8).map(|i| [i; 32]).collect(), ..base.clone() };
    assert!(matches!(with(many), Err(Error::BadListing(_))));
    let twice = ListingDetails { images: vec![[3; 32], [3; 32]], ..base.clone() };
    assert!(matches!(with(twice), Err(Error::BadListing(_))));
    for currency in ["usd", "US", "USDT"] {
        let price = Some(FiatPrice { currency: currency.into(), amount: 1, shipping: 0 });
        assert!(matches!(with(ListingDetails { price, ..base.clone() }), Err(Error::BadListing(_))), "{currency}");
    }
    let free = Some(FiatPrice { currency: "EUR".into(), amount: 0, shipping: 0 });
    assert!(matches!(with(ListingDetails { price: free, ..base.clone() }), Err(Error::BadListing(_))));
    let unlabeled = Listing { content: None, details: Some(base), ..v5() };
    assert!(matches!(unlabeled.resign(&secret()), Err(Error::BadListing(_))));

    let mut bytes = v6().encode().unwrap();
    // From the end: signature, price (tag, currency, amount, shipping), two images.
    let count_at = bytes.len() - 64 - (1 + 2 + 3 + 8 + 8) - 2 * 32 - 1;
    assert_eq!(bytes[count_at], 2);
    bytes[count_at] = (MAX_IMAGES + 1) as u8;
    assert!(Listing::decode(&bytes).is_err());
}

#[test]
fn resign_needs_the_store_key_and_checks_public_text() {
    let other = sign::secret_from_label("someone-else");
    assert_eq!(v6().resign(&other), Err(Error::StoreMismatch));
    let mut leaky = v6();
    leaky.description = "Call 503-555-0142.".into();
    assert!(matches!(leaky.resign(&secret()), Err(Error::PrivateText { .. })));
}

#[test]
fn fiat_scale_follows_the_currency() {
    assert_eq!(FiatPrice::scale("USD"), 100);
    assert_eq!(FiatPrice::scale("JPY"), 1);
}
