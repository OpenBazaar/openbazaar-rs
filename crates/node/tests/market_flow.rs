//! A seller and a buyer as two real `ob-node` processes: the seller opens a
//! store and lists, the buyer follows it and orders, and the seller answers.
//!
//! The in-process tests (`multinode_seller_tests.rs`) cover the same sale on
//! a mock chain, and step the mail loops by hand. Here nothing steps them:
//! the binary's own `serve` loops carry the order and its answer, through its
//! flag parsing and its HTTP guard. No payment is made.
//!
//! The loopback test runs with `cargo test`. The Tor test is ignored so that
//! stays offline:
//!
//! ```text
//! cargo test --locked -p ob-node --test market_flow -- --ignored --nocapture
//! ```

mod common;

use std::time::Duration;

use common::{chain_stub, timed, wait_for, RealNode};
use serde_json::{json, Value};

const PASSPHRASE: &str = "correct horse battery";

#[test]
fn two_real_nodes_trade_on_loopback() {
    let chain = chain_stub();
    let limit = Duration::from_secs(30);
    let seller = RealNode::start("seller", &chain, &[], limit);
    let buyer = RealNode::start("buyer", &chain, &[], limit);
    let listing = open_store(&seller);
    setup(&buyer);
    order_and_answer(&seller, &buyer, &seller.url, &listing, limit);
}

#[test]
#[ignore = "bootstraps two Arti clients; fails unless the order and its answer cross Tor"]
fn two_real_nodes_trade_over_tor() {
    let chain = chain_stub();
    // An onion takes 10 to 200 seconds to publish, and a letter that found it
    // unpublished waits out a doubling backoff before it is sent again.
    let limit = Duration::from_secs(480);
    let seller = RealNode::start("seller", &chain, &["--remote"], limit);
    let buyer = RealNode::start("buyer", &chain, &["--remote"], limit);
    let listing = open_store(&seller);
    setup(&buyer);
    let store = store_onion(&seller, limit);
    // The buyer asks for the answer at an onion only once its own Tor is up.
    store_onion(&buyer, limit);
    order_and_answer(&seller, &buyer, &format!("http://{store}"), &listing, limit);
}

/// The business outcome: what the seller's answer says, on both nodes.
///
/// This is the one place that knows it. Today a store with no finished sale
/// opens no sale for a direct order, so the buyer's order stops here. That is
/// a product gap, not the goal. When the new-store fix lands this becomes:
/// the order is "Awaiting payment" with a payment address, and the seller
/// lists the sale.
fn check_outcome(order: &Value, sales: &[Value]) {
    assert_eq!(order["status"], "Declined by the seller", "{order}");
    assert!(order["remote"]["reason"].as_str().unwrap().contains("moderated escrow"), "{order}");
    assert!(sales.is_empty(), "{sales:?}");
}

fn setup(node: &RealNode) {
    // The answer carries the new wallet's phrase, so it is not printed.
    let (status, _) = node.request("POST", "/setup", &json!({ "passphrase": PASSPHRASE }).to_string());
    assert_eq!(status, 200, "{} POST /setup", node.name);
}

/// A tiny PNG. The seed changes the pixel so two images do not share a hash.
fn png(seed: u8) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([seed, seed.wrapping_add(1), seed.wrapping_add(2), 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

fn unhex(value: &Value) -> Vec<u8> {
    data_encoding::HEXLOWER.decode(value.as_str().unwrap().as_bytes()).unwrap()
}

/// Set the node up, choose to sell, name the store, and publish one listing.
/// Checks what another node is shown. Returns the listing as the seller serves it.
fn open_store(seller: &RealNode) -> Value {
    setup(seller);
    let (status, role) = seller.request("PUT", "/store/role", r#"{"role":"seller"}"#);
    assert_eq!(status, 200, "{role}");
    let (_, avatar) = seller.upload(&png(1));
    let profile = json!({
        "name": "Brook and Pine",
        "tagline": "Small-batch ceramics",
        "about": "Pots and mugs thrown by hand.",
        "avatar": avatar["hash"],
        "shipsTo": ["US", "CA"],
        "shippingPolicy": "Ships in 3 days.",
    });
    let (status, saved) = seller.request("PUT", "/store/profile", &profile.to_string());
    assert_eq!(status, 200, "{saved}");
    let (_, image) = seller.upload(&png(2));
    let mug = json!({
        "title": "Hand-thrown mug",
        "description": "Stoneware, 12 oz, glazed in sea green.",
        "category": "Home",
        "kind": "physical",
        "shipFrom": "Portland",
        "stock": 5,
        "images": [image["hash"]],
        "currency": "SAT",
        "price": 25_000,
        "shipping": 3_000,
        "contentRating": "general"
    });
    let (status, listing) = seller.request("POST", "/store/listings", &mug.to_string());
    assert_eq!(status, 200, "{listing}");

    let (status, public) = seller.peer_get("/public/store");
    assert_eq!(status, 200, "{public}");
    let store = openbazaar_protocol::Store::decode(&unhex(&public["store"])).unwrap();
    store.verify().unwrap();
    assert_eq!(store.name, "Brook and Pine");
    let listings = public["listings"].as_array().unwrap();
    assert_eq!(listings.len(), 1, "{public}");
    let signed = openbazaar_protocol::Listing::decode(&unhex(&listings[0])).unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.content_hash().unwrap().to_hex(), listing["hash"].as_str().unwrap());
    listing
}

/// The node's public store address, `….onion`, once its Tor client is up.
fn store_onion(node: &RealNode, limit: Duration) -> String {
    wait_for(&format!("the {} node's store onion", node.name), limit, || {
        let (_, remote) = node.request("GET", "/remote", "");
        remote["saleOnion"].as_str().map(str::to_string).ok_or_else(|| remote["tor"].to_string())
    })
}

/// The buyer follows the store at `store_url`, orders the listing, and the
/// seller's answer comes back. Each wait takes at most `limit`.
fn order_and_answer(seller: &RealNode, buyer: &RealNode, store_url: &str, listing: &Value, limit: Duration) {
    let (status, follows) = timed("the buyer's follow", || buyer.request("POST", "/follows", &json!({ "url": store_url }).to_string()));
    assert_eq!(status, 200, "{follows}");
    let copy = wait_for("the buyer's copy of the listing", limit, || {
        let (_, catalog) = buyer.request("GET", "/catalog", "");
        let found = catalog["listings"].as_array().and_then(|all| all.iter().find(|item| item["hash"] == listing["hash"]).cloned());
        found.ok_or_else(|| buyer.request("GET", "/follows", "").1["follows"].to_string())
    });
    assert_eq!(copy["listingId"], listing["listingId"]);

    let body = json!({
        "shipTo": { "name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202" },
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = timed("the buyer's checkout", || buyer.request("POST", "/checkout", &body.to_string()));
    assert_eq!(status, 200, "{placed}");
    let path = format!("/orders/{}", placed["orders"][0]["orderId"].as_str().unwrap());
    let order_when = |ready: fn(&Value) -> bool| {
        let (_, order) = buyer.request("GET", &path, "");
        if ready(&order) { Ok(order) } else { Err(order.to_string()) }
    };
    wait_for("the order to reach the seller's node", limit, || order_when(|order| order["remote"]["delivered"] == true));

    // From here the test only reads. The seller's order loop sends the answer
    // and the buyer's node files it under the seller's key for this order.
    let answered = wait_for("the seller's answer", limit, || order_when(|order| order["remote"]["seller"].is_string()));
    assert_ne!(answered["status"], "Awaiting seller", "{answered}");
    assert_eq!(answered["remote"]["url"], store_url, "the order went to the address the buyer follows");
    let (status, sales) = seller.request("GET", "/sales", "");
    assert_eq!(status, 200, "{sales}");
    check_outcome(&answered, sales["sales"].as_array().unwrap());
}
