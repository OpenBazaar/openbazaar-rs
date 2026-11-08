//! One node setting up a store: role, storefront, images, and listings.

use serde_json::{json, Value};

use crate::harness::{open_node, request, unlock, MockChain, TempDir};
use crate::http::dispatch;
use crate::network::NetworkMode;
use crate::{EscrowConfig, Node};

/// A tiny PNG: the node checks the signature bytes, not the pixels.
pub(crate) fn png(seed: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&[seed; 32]);
    bytes
}

/// `POST /media` with raw bytes. Returns the status line and JSON body.
pub(crate) fn upload(node: &Node, bytes: &[u8]) -> (String, Value) {
    let mut raw = format!(
        "POST /media HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Ob-Pair: {}\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
        node.pairing_token(),
        bytes.len()
    )
    .into_bytes();
    raw.extend_from_slice(bytes);
    let response = String::from_utf8_lossy(&dispatch(node, &raw)).to_string();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.lines().next().unwrap().to_string(), serde_json::from_str(body).unwrap_or(Value::Null))
}

/// An unpaired, origin-free request, as another node sends one.
pub(crate) fn peer_request(node: &Node, path: &str) -> (String, Vec<u8>) {
    let raw = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    let response = dispatch(node, raw.as_bytes());
    let split = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    (head.lines().next().unwrap().to_string(), response[split + 4..].to_vec())
}

pub(crate) fn mug(image: &str) -> Value {
    json!({
        "title": "Hand-thrown mug",
        "description": "Stoneware, 12 oz, glazed in sea green.",
        "category": "Home",
        "kind": "physical",
        "shipFrom": "Portland",
        "stock": 5,
        "images": [image],
        "currency": "SAT",
        "price": 25_000,
        "shipping": 3_000,
        "contentRating": "general"
    })
}

/// Choose to sell, name the store, and publish one listing with one image.
/// Returns the listing as the node serves it.
pub(crate) fn open_store(node: &Node) -> Value {
    let (status, _) = request(node, "PUT", "/store/role", r#"{"role":"seller"}"#);
    assert!(status.contains("200"), "{status}");
    let (_, avatar) = upload(node, &png(1));
    let profile = json!({
        "name": "Brook and Pine",
        "tagline": "Small-batch ceramics",
        "about": "Pots and mugs thrown by hand.",
        "avatar": avatar["hash"],
        "shipsTo": ["US", "CA"],
        "shippingPolicy": "Ships in 3 days.",
    });
    let (status, saved) = request(node, "PUT", "/store/profile", &profile.to_string());
    assert!(status.contains("200"), "{status} {saved}");
    let (_, image) = upload(node, &png(2));
    let (status, listing) = request(node, "POST", "/store/listings", &mug(image["hash"].as_str().unwrap()).to_string());
    assert!(status.contains("200"), "{status} {listing}");
    listing
}

fn seller() -> (TempDir, Node) {
    let dir = TempDir::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &MockChain::new(), 1, EscrowConfig::default());
    (dir, node)
}

#[test]
fn a_new_node_has_no_role_until_the_user_chooses_one() {
    let (_dir, node) = seller();
    let (_, profile) = request(&node, "GET", "/store/profile", "");
    assert_eq!(profile["role"], Value::Null);
    assert_eq!(profile["listingCount"], 0);
    assert_eq!(profile["setUp"], false, "the default store is not set up");
    let (status, _) = request(&node, "PUT", "/store/role", r#"{"role":"merchant"}"#);
    assert!(status.contains("400"), "{status}");
    let (_, profile) = request(&node, "PUT", "/store/role", r#"{"role":"buyer"}"#);
    assert_eq!(profile["role"], "buyer");
    let (status, _) = peer_request(&node, "/public/store");
    assert!(status.contains("404"), "a buyer node publishes no store: {status}");
}

#[test]
fn the_storefront_is_signed_into_the_store_record() {
    let (_dir, node) = seller();
    open_store(&node);
    let record = node.own_record().unwrap();
    record.verify().unwrap();
    assert_eq!(record.name, "Brook and Pine");
    assert_eq!(request(&node, "GET", "/store/profile", "").1["setUp"], true);
    let details = record.details.unwrap();
    assert_eq!(details.profile.tagline, "Small-batch ceramics");
    assert_eq!(details.profile.ships_to.len(), 2);
    assert!(details.profile.avatar.is_some());
    let (_, profile) = request(&node, "GET", "/store/profile", "");
    assert_eq!((profile["name"].as_str(), profile["role"].as_str()), (Some("Brook and Pine"), Some("seller")));
    assert_eq!(profile["listingCount"], 1);
    assert_eq!(profile["escrowAvailable"], false);

    for (body, why) in [
        (json!({ "name": "" }), "an empty name"),
        (json!({ "avatar": "00".repeat(32) }), "an image that was never uploaded"),
        (json!({ "shipsTo": ["USA"] }), "a country code that is not two letters"),
        (json!({ "colour": "blue" }), "a field the storefront does not have"),
    ] {
        let (status, _) = request(&node, "PUT", "/store/profile", &body.to_string());
        assert!(status.contains("400"), "{why}: {status}");
    }
    assert_eq!(node.own_record().unwrap().name, "Brook and Pine", "a refused edit changes nothing");
}

#[test]
fn listings_keep_their_id_across_edits_and_old_addresses_still_resolve() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    let id = listing["listingId"].as_str().unwrap().to_string();
    let first = listing["hash"].as_str().unwrap().to_string();
    assert_eq!(listing["seller"], "Brook and Pine");
    assert_eq!(listing["images"].as_array().unwrap().len(), 1);

    let mut edit = crate::seller_store_tests::mug(listing["images"][0].as_str().unwrap());
    edit["price"] = json!(27_000);
    let (status, edited) = request(&node, "PUT", &format!("/store/listings/{id}"), &edit.to_string());
    assert!(status.contains("200"), "{status} {edited}");
    assert_eq!(edited["listingId"], id.as_str());
    assert_ne!(edited["hash"], first.as_str(), "an edit is a new signature, so a new address");
    assert_eq!(edited["priceSats"], 27_000);
    assert_eq!(node.own_listings().by_hash(&first).unwrap().price_sats, 27_000, "the old address finds the current version");

    let (_, catalog) = request(&node, "GET", "/catalog", "");
    let ours: Vec<&Value> = catalog["listings"].as_array().unwrap().iter().filter(|l| l["listingId"] == id.as_str()).collect();
    assert_eq!(ours.len(), 1, "the catalog serves one version");

    let (_, listings) = request(&node, "GET", "/store/listings", "");
    assert_eq!(listings["listings"].as_array().unwrap().len(), 1);

    let (status, _) = request(&node, "DELETE", &format!("/store/listings/{id}"), "");
    assert!(status.contains("200"), "{status}");
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert!(!catalog["listings"].as_array().unwrap().iter().any(|l| l["listingId"] == id.as_str()));
    let (status, _) = request(&node, "DELETE", &format!("/store/listings/{id}"), "");
    assert!(status.contains("404"), "{status}");
}

#[test]
fn listings_survive_a_restart_and_need_the_unlocked_wallet_to_change() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let listing = {
        let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
        open_store(&node)
    };
    let node = Node::open(&dir.0, crate::load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    let (_, catalog) = request(&node, "GET", "/catalog", "");
    assert!(catalog["listings"].as_array().unwrap().iter().any(|l| l["hash"] == listing["hash"]));
    let (status, _) = request(&node, "POST", "/store/listings", &mug(listing["images"][0].as_str().unwrap()).to_string());
    assert!(status.contains("423"), "a locked wallet cannot sign: {status}");
    let (status, _) = request(&node, "PUT", "/store/profile", r#"{"tagline":"New"}"#);
    assert!(status.contains("423"), "{status}");
    unlock(&node);
    let (status, _) = request(&node, "POST", "/store/listings", &mug(listing["images"][0].as_str().unwrap()).to_string());
    assert!(status.contains("200"), "{status}");
}

#[test]
fn bad_listings_are_refused_with_a_reason() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    let image = listing["images"][0].as_str().unwrap();
    let cases = [
        ("title", json!(""), "Enter a title"),
        ("price", json!(0), "above zero"),
        ("kind", json!("service"), "physical or a digital"),
        ("currency", json!("CHF"), "USD, EUR, GBP, or JPY"),
        ("images", json!(["11".repeat(32)]), "Upload each image"),
        ("stock", json!(-1), "from 0"),
        ("colour", json!("blue"), "not part of a listing"),
    ];
    for (key, value, reason) in cases {
        let mut body = mug(image);
        body[key] = value;
        let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
        assert!(status.contains("400"), "{key}: {status}");
        assert!(refused["error"].as_str().unwrap_or("").contains(reason), "{key}: {refused}");
    }
}

#[test]
fn a_fiat_price_is_signed_with_its_sats_at_todays_rate() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let mut body = mug(listing["images"][0].as_str().unwrap());
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(500);
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["price"], json!({ "currency": "USD", "amount": 2_500, "shipping": 500 }));
    assert_eq!((priced["priceSats"].as_u64(), priced["shippingSats"].as_u64()), (Some(25_000), Some(5_000)));

    node.fix_rates(&[]);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400") && refused["error"].as_str().unwrap().contains("USD rate"), "{status} {refused}");
}

#[test]
fn the_display_name_is_signed_up_to_64_bytes_and_empty_is_refused() {
    let (_dir, node) = seller();
    open_store(&node);
    let name = "N".repeat(64);
    let (status, saved) = request(&node, "PUT", "/store/profile", &json!({ "name": name }).to_string());
    assert!(status.contains("200"), "{status} {saved}");
    assert_eq!(saved["name"], name);
    let record = node.own_record().unwrap();
    record.verify().unwrap();
    assert_eq!(record.name, name);
    let listing = node.own_listings().entries[0].listing.clone();
    listing.verify().unwrap();
    assert_eq!(listing.seller, name, "buyers see the signed display name");

    let wide = "字".repeat(22);
    assert!(wide.chars().count() <= 64 && wide.len() > 64);
    for (body, why) in [
        (json!({ "name": "" }), "empty"),
        (json!({ "name": "   " }), "blank"),
        (json!({ "name": "N".repeat(65) }), "65 bytes"),
        (json!({ "name": wide }), "more than 64 bytes"),
    ] {
        let (status, refused) = request(&node, "PUT", "/store/profile", &body.to_string());
        assert!(status.contains("400"), "{why}: {status} {refused}");
        assert_eq!(node.own_record().unwrap().name, name, "{why} leaves the signed name");
    }
}

#[test]
fn renaming_the_store_signs_its_listings_again() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    let (status, _) = request(&node, "PUT", "/store/profile", r#"{"name":"Pine and Brook"}"#);
    assert!(status.contains("200"), "{status}");
    let own = node.own_listings();
    let current = &own.entries[0].listing;
    current.verify().unwrap();
    assert_eq!(current.seller, "Pine and Brook");
    assert_eq!(own.entries[0].superseded[0], listing["hash"].as_str().unwrap());
}

#[test]
fn an_own_listing_takes_a_content_label() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    let body = json!({ "target": listing["hash"], "rating": "sensitive", "tags": ["weapons"] }).to_string();
    let (status, labelled) = request(&node, "POST", "/content-labels", &body);
    assert!(status.contains("200"), "{status} {labelled}");
    let current = node.own_listings().entries[0].listing.clone();
    assert_eq!(current.content_hash().unwrap().to_hex(), labelled["hash"].as_str().unwrap());
    assert_eq!(current.content.unwrap().rating, openbazaar_protocol::ContentRating::Sensitive);
}

#[test]
fn other_nodes_see_only_a_selling_store_and_its_own_images() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    let (status, body) = peer_request(&node, "/public/store");
    assert!(status.contains("200"), "{status}");
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["listings"].as_array().unwrap().len(), 1);
    let image = listing["images"][0].as_str().unwrap();
    let (status, bytes) = peer_request(&node, &format!("/public/media/{image}"));
    assert!(status.contains("200"), "{status}");
    assert_eq!(bytes, png(2));

    let (_, stray) = upload(&node, &png(9));
    let (status, _) = peer_request(&node, &format!("/public/media/{}", stray["hash"].as_str().unwrap()));
    assert!(status.contains("404"), "an image no record names stays private: {status}");

    let raw = "GET /public/store HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\n\r\n";
    let response = String::from_utf8_lossy(&dispatch(&node, raw.as_bytes())).to_string();
    assert!(response.starts_with("HTTP/1.1 403"), "pages may not read it: {response}");

    request(&node, "PUT", "/store/role", r#"{"role":"buyer"}"#);
    let (status, _) = peer_request(&node, "/public/store");
    assert!(status.contains("404"), "{status}");
}

#[test]
fn a_scheduled_sale_is_signed_onto_the_listing_and_a_backwards_window_is_not() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let image = listing["images"][0].as_str().unwrap();
    // 20.00 on a 25.00 price is 2,000 basis points, the sale the editor saves.
    let sale = json!({ "offBps": 2_000, "startsOn": 20261101, "endsOn": 20261130 });
    let mut body = mug(image);
    body["title"] = json!("Sale mug");
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(0);
    body["commerce"] = json!({ "condition": "new", "sale": sale });
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["commerce"]["sale"], sale);
    let bytes = crate::escrow_mail::unhex(priced["record"].as_str().unwrap()).unwrap();
    let signed = openbazaar_protocol::Listing::decode(&bytes).unwrap();
    signed.verify().unwrap();
    assert_eq!(
        signed.commerce.unwrap().pricing.sale,
        Some(openbazaar_protocol::Sale { off_bps: 2_000, starts_on: 20261101, ends_on: 20261130 }),
        "the signed listing carries the sale the editor saved"
    );

    body["commerce"]["sale"]["endsOn"] = json!(20261101);
    body["commerce"]["sale"]["startsOn"] = json!(20261130);
    let before = node.own_listings().entries.len();
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("ends before it starts"), "{refused}");
    assert_eq!(node.own_listings().entries.len(), before, "an invalid window is not signed");
    let kept = &node.own_listings().entries[1].listing;
    assert_eq!(kept.commerce.as_ref().unwrap().pricing.sale.unwrap().ends_on, 20261130);
}

#[test]
fn a_package_is_signed_onto_the_listing_and_an_invalid_handling_time_is_not() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let image = listing["images"][0].as_str().unwrap();
    let mut body = mug(image);
    body["title"] = json!("Packed mug");
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(0);
    body["commerce"] = json!({
        "condition": "new",
        "shipping": {
            "weightG": 450,
            "dimsMm": [120, 80, 60],
            "handlingDays": 2,
            "freeOver": 5_000,
            "services": [{ "name": "Standard", "price": 500, "extraItem": 0, "daysMin": 2, "daysMax": 5, "regions": ["US", "CA"] }]
        }
    });
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["commerce"]["shipping"]["weightG"], 450);
    assert_eq!(priced["commerce"]["shipping"]["handlingDays"], 2);
    assert_eq!(priced["commerce"]["shipping"]["freeOver"], 5_000);
    assert_eq!(priced["commerce"]["shipping"]["services"][0]["regions"], json!(["CA", "US"]));
    let bytes = crate::escrow_mail::unhex(priced["record"].as_str().unwrap()).unwrap();
    let signed = openbazaar_protocol::Listing::decode(&bytes).unwrap();
    signed.verify().unwrap();
    let commerce = signed.commerce.unwrap();
    let shipping = &commerce.shipping;
    assert_eq!((shipping.weight_g, shipping.dims_mm, shipping.handling_days, shipping.free_over), (450, [120, 80, 60], 2, 5_000));
    let regions: Vec<_> = shipping.services[0].regions.iter().map(|code| code.as_str()).collect();
    assert_eq!(regions, ["CA", "US"], "the signed listing carries the package the editor saved");

    let before = node.own_listings().entries.len();
    body["commerce"]["shipping"]["handlingDays"] = json!(31);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("30"), "{refused}");
    body["commerce"]["shipping"]["handlingDays"] = json!(2);
    body["commerce"]["shipping"]["services"][0]["regions"] = json!(["USA"]);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("country code"), "{refused}");
    assert_eq!(node.own_listings().entries.len(), before, "an invalid package is not signed");
    assert_eq!(node.own_listings().entries[1].listing.commerce.as_ref().unwrap().shipping.handling_days, 2);
}

#[test]
fn specifics_and_returns_are_signed_onto_the_listing_and_an_invalid_value_is_not() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let image = listing["images"][0].as_str().unwrap();
    let mut body = mug(image);
    body["title"] = json!("Labeled mug");
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(0);
    body["commerce"] = json!({
        "condition": "new",
        "specifics": { "Brand": "Marlow", "upc": "036000291452" },
        "returns": { "kind": "days", "days": 30, "sellerPays": true }
    });
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["commerce"]["specifics"]["brand"], "Marlow");
    assert_eq!(priced["commerce"]["specifics"]["upc"], "036000291452");
    assert_eq!(priced["commerce"]["returns"], json!({ "kind": "days", "days": 30, "sellerPays": true }));
    let bytes = crate::escrow_mail::unhex(priced["record"].as_str().unwrap()).unwrap();
    let signed = openbazaar_protocol::Listing::decode(&bytes).unwrap();
    signed.verify().unwrap();
    let commerce = signed.commerce.unwrap();
    assert_eq!(commerce.specifics, vec![("brand".into(), "Marlow".into()), ("upc".into(), "036000291452".into())]);
    assert_eq!(commerce.returns, openbazaar_protocol::Returns::Days { days: 30, seller_pays: true }, "the signed listing carries the specifics and returns the editor saved");

    let before = node.own_listings().entries.len();
    body["commerce"]["returns"]["days"] = json!(0);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("365"), "{refused}");
    body["commerce"]["returns"]["days"] = json!(30);
    body["commerce"]["specifics"]["upc"] = json!("036000291453");
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("check digit"), "{refused}");
    assert_eq!(node.own_listings().entries.len(), before, "an invalid specific or returns window is not signed");
    assert_eq!(node.own_listings().entries[1].listing.commerce.as_ref().unwrap().returns, openbazaar_protocol::Returns::Days { days: 30, seller_pays: true });
}

#[test]
fn compare_at_and_offers_are_signed_onto_the_listing_and_an_invalid_compare_at_is_not() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let image = listing["images"][0].as_str().unwrap();
    // 32.00 on a 25.00 price, and offers on, the terms the editor saves.
    let mut body = mug(image);
    body["title"] = json!("Listed mug");
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(0);
    body["commerce"] = json!({ "condition": "new", "compareAt": 3_200, "acceptsOffers": true });
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["commerce"]["compareAt"], 3_200);
    assert_eq!(priced["commerce"]["acceptsOffers"], true);
    let bytes = crate::escrow_mail::unhex(priced["record"].as_str().unwrap()).unwrap();
    let signed = openbazaar_protocol::Listing::decode(&bytes).unwrap();
    signed.verify().unwrap();
    let pricing = signed.commerce.unwrap().pricing;
    assert_eq!((pricing.compare_at, pricing.accepts_offers), (3_200, true), "the signed listing carries the compare-at and offers flag the editor saved");

    let before = node.own_listings().entries.len();
    body["commerce"]["compareAt"] = json!(2_500);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("above"), "{refused}");
    body["commerce"]["compareAt"] = json!(3_200);
    body["commerce"]["acceptsOffers"] = json!("yes");
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("offers flag"), "{refused}");
    let kept = node.own_listings();
    assert_eq!(kept.entries.len(), before, "an invalid compare-at or offers flag is not signed");
    let pricing = &kept.entries[1].listing.commerce.as_ref().unwrap().pricing;
    assert_eq!((pricing.compare_at, pricing.accepts_offers), (3_200, true));
}

#[test]
fn availability_and_lot_size_are_signed_onto_the_listing_and_an_invalid_value_is_not() {
    let (_dir, node) = seller();
    let listing = open_store(&node);
    node.fix_rates(&[("USD", 100_000.0)]);
    let image = listing["images"][0].as_str().unwrap();
    // A lot of 5, pre-order shipping on 1 Dec 2026, the terms the editor saves.
    let mut body = mug(image);
    body["title"] = json!("Lot mug");
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(0);
    body["commerce"] = json!({
        "condition": "new",
        "lotSize": 5,
        "availability": { "kind": "pre-order", "shipsOn": 20261201 }
    });
    let (status, priced) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {priced}");
    assert_eq!(priced["commerce"]["lotSize"], 5);
    assert_eq!(priced["commerce"]["availability"], json!({ "kind": "pre-order", "shipsOn": 20261201 }));
    let bytes = crate::escrow_mail::unhex(priced["record"].as_str().unwrap()).unwrap();
    let signed = openbazaar_protocol::Listing::decode(&bytes).unwrap();
    signed.verify().unwrap();
    let purchase = signed.commerce.unwrap().purchase;
    assert_eq!(purchase.lot_size, 5);
    assert_eq!(purchase.availability, openbazaar_protocol::Availability::PreOrder { ships_on: 20261201 }, "the signed listing carries the availability and lot size the editor saved");

    let before = node.own_listings().entries.len();
    body["commerce"]["lotSize"] = json!(0);
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("one piece"), "{refused}");
    body["commerce"]["lotSize"] = json!(5);
    body["commerce"]["availability"]["kind"] = json!("paused");
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("Availability"), "{refused}");
    body["commerce"]["availability"] = json!({ "kind": "made-to-order", "days": 400 });
    let (status, refused) = request(&node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("400"), "{status} {refused}");
    assert!(refused["error"].as_str().unwrap_or("").contains("365"), "{refused}");
    let kept = node.own_listings();
    assert_eq!(kept.entries.len(), before, "an invalid availability or lot size is not signed");
    let purchase = kept.entries[1].listing.commerce.as_ref().unwrap().purchase;
    assert_eq!((purchase.lot_size, purchase.availability), (5, openbazaar_protocol::Availability::PreOrder { ships_on: 20261201 }));
}

#[test]
fn uploads_must_be_images() {
    let (_dir, node) = seller();
    let (status, _) = upload(&node, b"<svg onload=alert(1)>");
    assert!(status.contains("400"), "{status}");
    let (status, image) = upload(&node, &png(3));
    assert!(status.contains("200"), "{status}");
    assert_eq!(image["contentType"], "image/png");
    let raw = format!("POST /media HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n", 4);
    let response = String::from_utf8_lossy(&dispatch(&node, [raw.as_bytes(), b"\x89PNG"].concat().as_slice())).to_string();
    assert!(response.starts_with("HTTP/1.1 403"), "unpaired uploads are refused: {response}");
}
