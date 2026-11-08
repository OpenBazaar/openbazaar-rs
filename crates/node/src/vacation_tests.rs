//! Vacation mode: listings are served paused, the shelf count is kept, and
//! an order is refused without naming the buyer.

use openbazaar_protocol::VariantId;
use serde_json::{json, Value};

use crate::harness::{open_node, order, request, ship_to, unlock, MockChain, TempDir};
use crate::multinode_variant_tests::{deliver, peer};
use crate::network::NetworkMode;
use crate::own_listings::parse_id;
use crate::seller_store_tests::{mug, open_store};
use crate::vacation::AWAY_MESSAGE;
use crate::watch::DECLINED;
use crate::{EscrowConfig, Node};

fn shelf(node: &Node, listing_id: &str, variant: &str) -> u32 {
    let id = parse_id(listing_id).unwrap();
    let variant = if variant.is_empty() {
        VariantId::NONE
    } else {
        VariantId(
            crate::escrow_mail::unhex(variant)
                .unwrap()
                .try_into()
                .unwrap(),
        )
    };
    node.stock().shelf(&(id, variant), 0)
}

fn served<'a>(node: &'a Node, listing_id: &str) -> Value {
    let (_, catalog) = request(node, "GET", "/catalog", "");
    catalog["listings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listing| listing["listingId"] == listing_id)
        .cloned()
        .unwrap_or_else(|| panic!("missing {listing_id} in {catalog}"))
}

fn vacation(node: &Node, away: bool) -> (String, Value) {
    request(
        node,
        "PUT",
        "/store/vacation",
        &json!({ "away": away }).to_string(),
    )
}

#[test]
fn vacation_pauses_listings_keeps_the_shelf_and_names_no_buyer() {
    let (_dir, node) = {
        let dir = TempDir::new();
        let node = open_node(
            &dir.0,
            NetworkMode::Testnet4,
            &MockChain::new(),
            1,
            EscrowConfig::default(),
        );
        (dir, node)
    };
    node.fix_rates(&[("USD", 100_000.0)]);
    let mug_listing = open_store(&node);
    let mug_id = mug_listing["listingId"].as_str().unwrap();
    let image = mug_listing["images"][0].as_str().unwrap();
    let mut preorder = mug(image);
    preorder["title"] = json!("Bowl, ships in December");
    preorder["currency"] = json!("USD");
    preorder["price"] = json!(4_000);
    preorder["shipping"] = json!(0);
    preorder["stock"] = json!(4);
    preorder["commerce"] = json!({
        "axes": [{ "name": "Glaze", "values": ["Sea"] }],
        "variants": [{ "options": ["Sea"], "stock": 2 }],
        "availability": { "kind": "pre-order", "shipsOn": 20261201 },
    });
    let (status, bowl) = request(&node, "POST", "/store/listings", &preorder.to_string());
    assert!(status.contains("200"), "{status} {bowl}");
    let bowl_id = bowl["listingId"].as_str().unwrap().to_string();
    let sea = bowl["commerce"]["variants"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(bowl["commerce"]["availability"]["kind"], "pre-order");
    let store_before = node.own_record().unwrap().encode().unwrap();

    let (status, refused) = request(&node, "PUT", "/store/profile", r#"{"away":true}"#);
    assert!(
        status.contains("400"),
        "the signed storefront has no vacation field: {status} {refused}"
    );
    let (status, refused) = request(
        &node,
        "PUT",
        "/store/vacation",
        r#"{"away":true,"note":"in Paris until June"}"#,
    );
    assert!(
        status.contains("400"),
        "a vacation note could locate the seller: {status} {refused}"
    );
    assert_eq!(request(&node, "GET", "/store/profile", "").1["away"], false);

    let (status, profile) = vacation(&node, true);
    assert!(status.contains("200"), "{status} {profile}");
    assert_eq!(profile["away"], true);
    assert!(
        profile.get("vacationAvailability").is_none(),
        "the remembered availability is not served"
    );
    assert!(profile.get("note").is_none());
    assert_eq!(
        node.own_record().unwrap().encode().unwrap(),
        store_before,
        "vacation is not a new store record"
    );

    let paused_mug = served(&node, mug_id);
    assert_eq!(paused_mug["stock"], 0);
    assert!(
        paused_mug.get("commerce").is_none(),
        "a listing without commerce stays that way"
    );
    assert_eq!(
        shelf(&node, mug_id, ""),
        5,
        "the mug's shelf survives the pause"
    );
    let paused_bowl = served(&node, &bowl_id);
    assert_eq!(paused_bowl["stock"], 0);
    assert_eq!(
        paused_bowl["commerce"]["availability"],
        json!({ "kind": "away" })
    );
    assert_eq!(paused_bowl["commerce"]["variants"][0]["stock"], 0);
    assert_eq!(
        shelf(&node, &bowl_id, &sea),
        2,
        "the variant shelf survives the pause"
    );

    assert_eq!(
        node.watch_stock(),
        0,
        "the stock watcher must not publish the shelf while the store is away"
    );
    assert_eq!(served(&node, mug_id)["stock"], 0);
    assert_eq!(shelf(&node, mug_id, ""), 5);

    let mut echo = mug(image);
    echo["title"] = json!("Hand-thrown mug, glazed");
    echo["stock"] = json!(0);
    let (status, edited) = request(
        &node,
        "PUT",
        &format!("/store/listings/{mug_id}"),
        &echo.to_string(),
    );
    assert!(status.contains("200"), "{status} {edited}");
    assert_eq!(edited["title"], "Hand-thrown mug, glazed");
    assert_eq!(
        edited["stock"], 0,
        "an edit while away is still served paused"
    );
    assert_eq!(
        shelf(&node, mug_id, ""),
        5,
        "echoing the paused stock 0 does not wipe the shelf"
    );

    echo["stock"] = json!(8);
    let (status, edited) = request(
        &node,
        "PUT",
        &format!("/store/listings/{mug_id}"),
        &echo.to_string(),
    );
    assert!(status.contains("200"), "{status} {edited}");
    assert_eq!(edited["stock"], 0);
    assert_eq!(
        shelf(&node, mug_id, ""),
        8,
        "a real stock edit is kept for when the store opens"
    );

    let (status, again) = vacation(&node, true);
    assert!(status.contains("200"), "{status} {again}");
    assert_eq!(
        shelf(&node, mug_id, ""),
        8,
        "pausing again must not pin the published stock of 0"
    );
    assert_eq!(shelf(&node, &bowl_id, &sea), 2);

    let (status, open) = vacation(&node, false);
    assert!(status.contains("200"), "{status} {open}");
    assert_eq!(open["away"], false);
    let mug_back = served(&node, mug_id);
    assert_eq!(mug_back["stock"], 8);
    assert_eq!(mug_back["title"], "Hand-thrown mug, glazed");
    let bowl_back = served(&node, &bowl_id);
    assert_eq!(bowl_back["stock"], 2);
    assert_eq!(bowl_back["commerce"]["variants"][0]["stock"], 2);
    assert_eq!(
        bowl_back["commerce"]["availability"],
        json!({ "kind": "pre-order", "shipsOn": 20261201 })
    );
    assert_eq!(node.own_record().unwrap().encode().unwrap(), store_before);
}

#[test]
fn vacation_survives_a_restart_and_needs_the_unlocked_wallet() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let listing = {
        let node = open_node(
            &dir.0,
            NetworkMode::Testnet4,
            &chain,
            1,
            EscrowConfig::default(),
        );
        let listing = open_store(&node);
        let (status, _) = vacation(&node, true);
        assert!(status.contains("200"), "{status}");
        listing
    };
    let id = listing["listingId"].as_str().unwrap();
    let node = Node::open(&dir.0, crate::load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    assert_eq!(request(&node, "GET", "/store/profile", "").1["away"], true);
    assert_eq!(served(&node, id)["stock"], 0);
    let (status, _) = vacation(&node, false);
    assert!(
        status.contains("423"),
        "a locked wallet cannot sign the listings back: {status}"
    );
    assert_eq!(served(&node, id)["stock"], 0);
    assert_eq!(shelf(&node, id, ""), 5);
    unlock(&node);
    let (status, profile) = vacation(&node, false);
    assert!(status.contains("200"), "{status} {profile}");
    assert_eq!(profile["away"], false);
    assert_eq!(served(&node, id)["stock"], 5);
}

#[test]
fn an_order_during_vacation_is_declined_without_naming_the_buyer() {
    let chain = MockChain::new();
    let seller = peer(&chain);
    let buyer = peer(&chain);
    let listing = open_store(&seller.node);
    let hash = listing["hash"].as_str().unwrap();
    let (status, follows) = request(
        &buyer.node,
        "POST",
        "/follows",
        &json!({ "url": seller.url }).to_string(),
    );
    assert!(status.contains("200"), "{status} {follows}");
    let (status, _) = vacation(&seller.node, true);
    assert!(status.contains("200"), "{status}");

    let body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": hash, "quantity": 1 }] }],
    });
    let (status, placed) = request(&buyer.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {placed}");
    let id = placed["orders"][0]["orderId"].as_str().unwrap();
    deliver(&seller, &buyer);
    let declined = order(&buyer.node, id);
    assert_eq!(declined["status"], DECLINED, "{declined}");
    assert_eq!(declined["remote"]["reason"], AWAY_MESSAGE, "{declined}");
    let reason = declined["remote"]["reason"].as_str().unwrap();
    assert!(
        !reason.contains("14 Larch"),
        "the refusal must not repeat the buyer's address"
    );
    assert!(
        !reason.contains(&buyer.node.own_store.to_hex()),
        "the refusal must not name the buyer's store"
    );
    let (_, sales) = request(&seller.node, "GET", "/sales", "");
    assert!(
        sales["sales"].as_array().unwrap().is_empty(),
        "a refused order is not a sale: {sales}"
    );
}
