//! A listing with variants, a shipping service and a coupon, sold between
//! two nodes: the buyer's node prices the package with the shared rules, the
//! seller's node holds the units, applies the coupon and quotes in fiat, a
//! second order for held units is declined before anything is paid, and the
//! payment takes the units off the shelf, re-signs the listing and brings
//! the buyer a receipt for its per-order review key.

use serde_json::{json, Value};

use crate::harness::{fund_node_wallet, order, pay, request, ship_to, MockChain, Peer};
use crate::seller_store_tests::{mug, open_store};
use crate::watch::{AWAITING_PAYMENT, DECLINED, PAID};
use crate::EscrowConfig;

pub(crate) fn peer(chain: &MockChain) -> Peer {
    Peer::spawn(chain, |url| EscrowConfig { mailbox_url: Some(url), ..EscrowConfig::default() })
}

pub(crate) fn deliver(seller: &Peer, buyer: &Peer) {
    for _ in 0..3 {
        buyer.node.flush_order_mail(true);
        seller.node.flush_order_mail(true);
    }
}

/// A mug in two glazes: Sea at the listing's $25.00 (2 left), Sand at $30.00 (3 left).
pub(crate) fn glazed_mug(seller: &Peer) -> Value {
    let first = open_store(&seller.node);
    let mut body = mug(first["images"][0].as_str().unwrap());
    body["currency"] = json!("USD");
    body["price"] = json!(2_500);
    body["shipping"] = json!(500);
    body["commerce"] = json!({
        "condition": "new",
        "axes": [{ "name": "Glaze", "values": ["Sea", "Sand"] }],
        "variants": [{ "options": ["Sea"], "stock": 2 }, { "options": ["Sand"], "stock": 3, "price": 3_000 }],
        "maxPerOrder": 3,
        "shipping": { "services": [{ "name": "Post", "price": 500, "extraItem": 100, "daysMin": 2, "daysMax": 5 }] },
    });
    let (status, listing) = request(&seller.node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {listing}");
    assert_eq!(listing["stock"], 5, "the listing's stock is its variants' total");
    listing
}

pub(crate) fn checkout(buyer: &Peer, lines: Value, coupon: &str) -> Value {
    let body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "direct", "lines": lines, "service": 0, "coupon": coupon }],
    });
    let (status, value) = request(&buyer.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    value["orders"][0].clone()
}

#[test]
fn two_variants_ship_together_with_a_coupon_and_leave_the_shelf_when_paid() {
    let chain = MockChain::new();
    let (seller, buyer) = (peer(&chain), peer(&chain));
    for node in [&seller.node, &buyer.node] {
        node.fix_rates(&[("USD", 100_000.0)]);
    }
    let listing = glazed_mug(&seller);
    let hash = listing["hash"].as_str().unwrap().to_string();
    let sea = listing["commerce"]["variants"][0]["id"].as_str().unwrap().to_string();
    let sand = listing["commerce"]["variants"][1]["id"].as_str().unwrap().to_string();
    let coupon = json!({ "code": "save10", "percentBps": 1_000, "currency": "USD", "startsOn": 20_200_101, "endsOn": 20_991_231 });
    let (status, made) = request(&seller.node, "POST", "/coupons", &coupon.to_string());
    assert!(status.contains("200"), "{status} {made}");
    assert_eq!(made["code"], "SAVE10");
    let (status, follows) = request(&buyer.node, "POST", "/follows", &json!({ "url": seller.url }).to_string());
    assert!(status.contains("200"), "{status} {follows}");

    let lines = json!([
        { "listingHash": hash, "quantity": 2, "variant": sea },
        { "listingHash": hash, "quantity": 1, "variant": sand },
    ]);
    let placed = checkout(&buyer, lines, "save10");
    let id = placed["orderId"].as_str().unwrap().to_string();
    assert_eq!(
        (placed["itemsSats"].as_u64(), placed["shippingSats"].as_u64()),
        (Some(80_000), Some(7_000)),
        "$80.00 of mugs and $5.00 + 2 × $1.00 shipping at $100,000 a bitcoin, before the seller's coupon"
    );
    assert_eq!(placed["lines"][1]["variantLabel"], "Sand");

    let (_, sales) = request(&seller.node, "GET", "/sales", "");
    let sale = sales["sales"][0].clone();
    assert_eq!(sale["quote"]["couponOff"], 800, "{sale}");
    assert_eq!((sale["itemsSats"].as_u64(), sale["totalSats"].as_u64()), (Some(72_000), Some(79_000)));
    let labels: Vec<&str> = sale["lines"].as_array().unwrap().iter().map(|l| l["variantLabel"].as_str().unwrap()).collect();
    assert_eq!(labels, ["Sea", "Sand"]);

    deliver(&seller, &buyer);
    let terms = order(&buyer.node, &id);
    assert_eq!(terms["status"], AWAITING_PAYMENT, "{terms}");
    assert_eq!(terms["totalSats"], 79_000);
    assert_eq!(terms["remote"]["sellerQuote"]["total"], 7_900);

    let late = checkout(&buyer, json!([{ "listingHash": hash, "quantity": 1, "variant": sea }]), "");
    deliver(&seller, &buyer);
    let declined = order(&buyer.node, late["orderId"].as_str().unwrap());
    assert_eq!(declined["status"], DECLINED, "both Sea mugs are held for the first order: {declined}");
    assert!(declined["remote"]["reason"].as_str().unwrap().contains("sold out"), "{declined}");

    fund_node_wallet(&buyer.node, &chain, 200_000);
    let (status, paid) = pay(&buyer.node, &id, "{}");
    assert!(status.contains("202"), "{status} {paid}");
    chain.mine();
    seller.node.watch_once().unwrap();
    buyer.node.watch_once().unwrap();
    let (_, sales) = request(&seller.node, "GET", "/sales", "");
    assert_eq!(sales["sales"].as_array().unwrap().iter().find(|s| s["id"] == sale["id"]).unwrap()["status"], PAID);

    let (_, own) = request(&seller.node, "GET", "/store/listings", "");
    let now = own["listings"].as_array().unwrap().iter().find(|l| l["listingId"] == listing["listingId"]).unwrap().clone();
    assert_ne!(now["hash"], listing["hash"], "the sale re-signed the listing");
    assert_eq!(now["stock"], 2);
    let stock: Vec<u64> = now["commerce"]["variants"].as_array().unwrap().iter().map(|v| v["stock"].as_u64().unwrap()).collect();
    assert_eq!(stock, [0, 2]);

    deliver(&seller, &buyer);
    let kept = order(&buyer.node, &id);
    assert_eq!(kept["status"], PAID);
    assert_eq!(kept["remote"]["receipts"].as_array().map(Vec::len), Some(1), "one receipt for the one listing: {kept}");
    let (_, coupons) = request(&seller.node, "GET", "/coupons", "");
    assert_eq!(coupons["coupons"][0]["uses"], 1);
}
