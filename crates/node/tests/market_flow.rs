//! A seller and a buyer as two real `ob-node` processes: the seller opens a
//! new store and lists, the buyer follows it and orders, the seller names an
//! address, the buyer pays it, the seller ships, and the buyer confirms.
//!
//! The in-process tests (`multinode_seller_tests.rs`) cover the same sale on
//! a mock chain, and step the mail loops by hand. Here nothing steps them:
//! the binary's own `serve` loops carry the order and every answer, through
//! its flag parsing and its HTTP guard, and its own watchers ask a stand-in
//! Esplora server on loopback. Someone outside the nodes pays the address:
//! the stand-in serves no wallet scan, so the buyer's node wallet is empty.
//!
//! The loopback test runs with `cargo test`. The Tor test is ignored so that
//! stays offline:
//!
//! ```text
//! cargo test --locked -p ob-node --test market_flow -- --ignored --nocapture
//! ```

mod common;

use std::time::Duration;

use common::esplora::Esplora;
use common::{timed, wait_for, RealNode};
use serde_json::{json, Value};

const PASSPHRASE: &str = "correct horse battery";

#[test]
fn two_real_nodes_trade_on_loopback() {
    let chain = Esplora::start();
    let limit = Duration::from_secs(30);
    let seller = RealNode::start("seller", &chain.url, &[], limit);
    let buyer = RealNode::start("buyer", &chain.url, &[], limit);
    let listing = open_store(&seller);
    setup(&buyer);
    sale_to_completed(&seller, &buyer, &chain, &seller.url, &listing, limit);
    for node in [&seller, &buyer] {
        let client = node.data_dir().join("tor-client");
        assert!(!client.exists(), "the {} node opened the Tor client that dials the introductions", node.name);
    }
}

#[test]
#[ignore = "bootstraps two Arti clients; fails unless the order and every answer cross Tor"]
fn two_real_nodes_trade_over_tor() {
    // The chain stand-in stays on loopback; only the letters cross Tor.
    let chain = Esplora::start();
    // An onion takes 10 to 200 seconds to publish, and a letter that found it
    // unpublished waits out a doubling backoff before it is sent again.
    let limit = Duration::from_secs(480);
    let seller = RealNode::start("seller", &chain.url, &["--remote"], limit);
    let buyer = RealNode::start("buyer", &chain.url, &["--remote"], limit);
    let listing = open_store(&seller);
    setup(&buyer);
    let store = store_onion(&seller, limit);
    // The buyer asks for the answer at an onion only once its own Tor is up.
    store_onion(&buyer, limit);
    sale_to_completed(&seller, &buyer, &chain, &format!("http://{store}"), &listing, limit);
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

/// The buyer follows the new store at `store_url` and orders the listing; the
/// seller names an address; it is paid on the stand-in chain and both nodes
/// call the order paid; the seller ships; the buyer confirms it arrived.
/// Each wait takes at most `limit`.
fn sale_to_completed(seller: &RealNode, buyer: &RealNode, chain: &Esplora, store_url: &str, listing: &Value, limit: Duration) {
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
    let sale_when = |ready: fn(&Value) -> bool| {
        let (_, sales) = seller.request("GET", "/sales", "");
        match sales["sales"].as_array().map(Vec::as_slice) {
            Some([sale]) if ready(sale) => Ok(sale.clone()),
            _ => Err(sales.to_string()),
        }
    };
    wait_for("the order to reach the seller's node", limit, || order_when(|order| order["remote"]["delivered"] == true));

    // The seller's order loop sends the terms and the buyer's node files them
    // under the seller's key for this order. The store has no finished sale.
    let terms = wait_for("the seller's terms", limit, || order_when(|order| order["remote"]["seller"].is_string()));
    assert_eq!(terms["status"], "Awaiting payment", "a new store takes a direct order: {terms}");
    assert_eq!(terms["remote"]["url"], store_url, "the order went to the address the buyer follows");
    let total = 25_000 + 3_000;
    assert_eq!(terms["totalSats"], total, "{terms}");
    let address = terms["payment"]["address"].as_str().unwrap();
    let sale = wait_for("the seller's sale", limit, || sale_when(|_| true));
    assert_eq!((&sale["status"], &sale["address"], &sale["totalSats"]), (&json!("Awaiting payment"), &json!(address), &json!(total)), "{sale}");
    assert!(sale["shipTo"].as_str().unwrap().contains("14 Larch Lane"), "{sale}");

    // Each node's own watcher asks the chain: first the payment, then its block.
    chain.pay(address, total);
    wait_for("the buyer's node to see the payment", limit, || order_when(|order| order["status"] == "Payment seen"));
    wait_for("the seller's node to see the payment", limit, || sale_when(|sale| sale["status"] == "Payment seen"));
    chain.mine();
    let paid = wait_for("the seller's node to call the sale paid", limit, || sale_when(|sale| sale["status"] == "Paid"));
    assert_eq!(paid["receivedSats"], total, "{paid}");
    let paid = wait_for("the buyer's node to hear the seller was paid", limit, || {
        order_when(|order| order["status"] == "Paid" && order["remote"]["sellerSawPayment"] == true)
    });
    assert_eq!(paid["receivedSats"], total, "{paid}");

    let fulfill = json!({ "carrier": "USPS", "tracking": "9400 1000 0000 0000 0000 00" });
    let (status, shipped) = seller.request("POST", &format!("/sales/{}/fulfill", sale["id"].as_str().unwrap()), &fulfill.to_string());
    assert_eq!((status, &shipped["status"]), (200, &json!("Shipped")), "{shipped}");
    let arriving = wait_for("the buyer's node to hear it shipped", limit, || order_when(|order| order["status"] == "Shipped"));
    assert_eq!(arriving["remote"]["fulfillment"]["tracking"], fulfill["tracking"], "{arriving}");

    let (status, done) = timed("the buyer's confirmation", || buyer.request("POST", &format!("{path}/complete"), ""));
    assert_eq!((status, &done["status"]), (200, &json!("Completed")), "{done}");
    wait_for("the seller's node to hear it arrived", limit, || sale_when(|sale| sale["status"] == "Completed"));
    wait_for("the seller's last answer", limit, || order_when(|order| order["remote"]["sellerCompleted"] == true));
}
