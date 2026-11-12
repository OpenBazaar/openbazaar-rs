//! A Zcash sale between two nodes, on a mock chain of real notes. Only the seller's node has an indexer.

use std::sync::Arc;

use openbazaar_zec::amount::zatoshi_for_sats;
use openbazaar_zec::mock::MockChain as ZecChainMock;
use openbazaar_zec::{ViewKey, ZecNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

use super::checkout::OTHER_NODE;
use super::own::NotReady;
use super::own_tests::{mock, switch};
use super::sale::{follow_seller, payment_from_terms, NOT_TAKEN, NO_ZEC_TERMS};
use crate::harness::{open_node, order, request, ship_to, unreachable_escrow, MockChain, TempDir};
use crate::multinode_seller_tests::Pair;
use crate::network::NetworkMode;
use crate::seller_store_tests::open_store;
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, DECLINED, PAID, PAYMENT_SEEN, SHIPPED};

const SATS_PER_ZEC: u64 = 1_229_360;
/// One of the listing (25,000 sats) and its shipping.
const PRICE: u64 = 28_000;

struct Shop {
    pair: Pair,
    zec: Arc<ZecChainMock>,
}

impl Shop {
    /// A seller whose own store takes Zcash, and a buyer with no indexer that follows it.
    fn open() -> Self {
        let (pair, zec) = (Pair::new(), mock(NetworkMode::Testnet4));
        let seller = &pair.seller.node;
        seller.set_zec_chain(zec.clone()).unwrap();
        seller.zec.rate.set_for_tests(Some(SATS_PER_ZEC));
        seller.zec_sync_once().unwrap();
        assert!(switch(seller, true).0.contains("200"));
        pair.buyer.node.replicate_once();
        Self { pair, zec }
    }

    /// Check out one of the listing in Zcash on the buyer's node, and carry the mail both ways. Returns the order.
    fn order(&self) -> Value {
        let (status, placed) = place(&self.pair);
        assert!(status.contains("200"), "{status} {placed}");
        assert_eq!(placed["orders"][0]["status"], AWAITING_SELLER);
        self.pair.deliver();
        order(&self.pair.buyer.node, placed["orders"][0]["orderId"].as_str().unwrap())
    }

    /// One round of the seller's Zcash wallet, then the mail. Returns the seller's sale and the buyer's order.
    fn round(&self, id: &str) -> (Value, Value) {
        self.pair.seller.node.zec_sync_once().unwrap();
        self.pair.deliver();
        (self.pair.sales().remove(0), order(&self.pair.buyer.node, id))
    }
}

fn place(pair: &Pair) -> (String, Value) {
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    let listing = catalog["listings"].as_array().unwrap().iter().find(|listing| listing["seller"] == "Brook and Pine").cloned().unwrap();
    assert_eq!((&listing["acceptsZec"], &catalog["zec"]["enabled"]), (&json!(true), &json!(false)), "offered with no indexer on the buyer's node");
    let package = json!({ "settlement": "direct", "rail": "zec", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] });
    request(&pair.buyer.node, "POST", "/checkout", &json!({ "shipTo": ship_to(), "packages": [package] }).to_string())
}

fn zatoshi(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}

/// Status, zatoshi received and confirmations: the same on the seller's sale and the buyer's order.
fn progress(of: &Value) -> (&str, u64, u64) {
    (of["status"].as_str().unwrap(), zatoshi(&of["payment"]["receivedZatoshi"]), of["confirmations"].as_u64().unwrap())
}

#[test]
fn a_zcash_sale_is_priced_by_the_seller_and_paid_on_both_nodes() {
    let shop = Shop::open();
    let placed = shop.order();
    let id = placed["id"].as_str().unwrap();
    let due = zatoshi_for_sats(PRICE, SATS_PER_ZEC).unwrap();

    // The seller's terms: its rate, its amount, a fresh address of the own store's wallet.
    let payment = &placed["payment"];
    assert_eq!((&placed["status"], &placed["totalSats"]), (&json!(AWAITING_PAYMENT), &json!(PRICE)), "{placed}");
    assert_eq!((&payment["rail"], &payment["network"], &payment["pool"]), (&json!("zec"), &json!("testnet"), &json!("ironwood")));
    assert_eq!((&payment["satsPerZec"], &payment["priceSats"], zatoshi(&payment["amountZatoshi"])), (&json!(SATS_PER_ZEC), &json!(PRICE), due));
    assert!(payment["address"].is_null(), "no bitcoin address: {payment}");
    let now = crate::buyer_orders::now_secs();
    assert!((now + 3590..=now + 3600).contains(&payment["expiresAt"].as_u64().unwrap()), "the quote lasts an hour");
    let sale = shop.pair.sales().remove(0);
    let sale_id = sale["id"].as_str().unwrap().to_string();
    assert_eq!((&sale["status"], &sale["totalSats"], &sale["address"]), (&json!(AWAITING_PAYMENT), &json!(PRICE), &Value::Null));
    assert!(shop.pair.seller.node.zec_needs_sync(), "an unpaid Zcash sale keeps the seller's sync on the fast pace");
    let address = payment["zcashAddress"].as_str().unwrap();
    {
        let slot = shop.pair.seller.node.zec.own();
        let index = sale["payment"]["addressIndex"].as_u64().unwrap() as u32;
        assert_eq!(slot.wallet.as_ref().unwrap().keys().address(index), address, "of the own store's wallet");
    }

    // Half of it: both sides record it and keep waiting. The bitcoin watchers leave the sale alone.
    shop.zec.pay(address, due / 2);
    shop.zec.mine(1);
    shop.pair.mine();
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (AWAITING_PAYMENT, due / 2, 1));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert_eq!(zatoshi(&bought["payment"]["remainingZatoshi"]), due - due / 2);

    shop.zec.pay(address, due - due / 2);
    shop.zec.mine(CONFIRMATIONS - 1);
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (PAYMENT_SEEN, due, CONFIRMATIONS - 1));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert!(bought["remote"]["sellerSawPayment"].is_null());

    shop.zec.mine(1);
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (PAID, due, CONFIRMATIONS));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert_eq!((&bought["remote"]["sellerSawPayment"], bought["txids"].as_array().unwrap().len()), (&json!(true), 2));
    assert!(!shop.pair.seller.node.zec_needs_sync(), "paid, caught up");

    // From here it is any sale.
    let (status, shipped) = request(&shop.pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), &json!({ "carrier": "USPS" }).to_string());
    assert!(status.contains("200") && shipped["status"] == SHIPPED, "{status} {shipped}");
    shop.pair.deliver();
    assert_eq!(order(&shop.pair.buyer.node, id)["status"], SHIPPED);
}

#[test]
fn a_zcash_quote_between_nodes_lapses_unless_the_seller_saw_it_paid_in_time() {
    let shop = Shop::open();
    let (late, seen) = (shop.order(), shop.order());
    let (late_id, seen_id) = (late["id"].as_str().unwrap(), seen["id"].as_str().unwrap());
    let due = zatoshi(&seen["payment"]["amountZatoshi"]);
    let pay = |order: &Value| shop.zec.pay(order["payment"]["zcashAddress"].as_str().unwrap(), due);
    let of = |id: &str| order(&shop.pair.buyer.node, id);

    // One is paid in time. Then the hour passes on the seller's node and a reorg drops that payment.
    pay(&seen);
    shop.zec.mine(3);
    shop.round(seen_id);
    assert_eq!((progress(&of(seen_id)), &of(seen_id)["payment"]["seenInTime"]), ((PAYMENT_SEEN, due, 3), &json!(true)));
    for sale in shop.pair.sales() {
        shop.pair.seller.node.sales().update(sale["id"].as_str().unwrap(), |sale| sale["payment"]["expiresAt"] = json!(1)).unwrap();
    }
    shop.zec.reorg(3);
    shop.zec.mine(4);
    shop.round(seen_id);
    let gone = of(seen_id);
    assert_eq!((progress(&gone), &gone["payment"]["expired"], &gone["payment"]["seenInTime"]), ((AWAITING_PAYMENT, 0, 0), &json!(false), &json!(true)), "{gone}");
    assert_eq!(of(late_id)["payment"]["expired"], true, "the seller told the buyer the unpaid quote lapsed");

    // Both are paid now. The one seen in time is paid; the one first paid after the hour is recorded on both nodes, not paid.
    pay(&seen);
    pay(&late);
    shop.zec.mine(CONFIRMATIONS);
    shop.round(seen_id);
    assert_eq!(progress(&of(seen_id)), (PAID, due, CONFIRMATIONS));
    let late = of(late_id);
    assert_eq!((progress(&late), &late["payment"]["expired"]), ((AWAITING_PAYMENT, due, CONFIRMATIONS), &json!(true)), "{late}");
    let statuses: Vec<Value> = shop.pair.sales().iter().map(|sale| sale["status"].clone()).collect();
    assert!(statuses.contains(&json!(PAID)) && statuses.contains(&json!(AWAITING_PAYMENT)), "{statuses:?}");
}

#[test]
fn the_seller_declines_a_zcash_order_it_cannot_take_and_says_why() {
    let (pair, zec) = (Pair::new(), mock(NetworkMode::Testnet4));
    let seller = &pair.seller.node;
    // The store's record takes Zcash, as after a restart without --zec-indexer.
    let secret = seller.store_secret(&seller.own_store).expect("unlocked");
    let rails = crate::own_store::Rails { zec: true, ..crate::own_store::Rails::default() };
    seller.publish_own(openbazaar_protocol::ContentRating::General, rails, &secret).unwrap();
    pair.buyer.node.replicate_once();
    let declined = |why: &str| {
        let (status, placed) = place(&pair);
        assert!(status.contains("200"), "{status} {placed}");
        pair.deliver();
        let order = order(&pair.buyer.node, placed["orders"][0]["orderId"].as_str().unwrap());
        assert_eq!((&order["status"], &order["remote"]["reason"]), (&json!(DECLINED), &json!(why)), "{order}");
        assert!(order["payment"].is_null() && pair.sales().is_empty(), "nothing to pay, and no sale was opened");
    };
    declined(NotReady::Off.buyer());
    seller.set_zec_chain(zec).unwrap();
    declined(NotReady::NotSynced.buyer());
    seller.zec_sync_once().unwrap();
    declined(NotReady::NoPrice.buyer());
    seller.zec.rate.set_for_tests(Some(SATS_PER_ZEC));
    // The seller switches Zcash off; the buyer's copy of the record is older.
    assert!(switch(seller, false).0.contains("200"));
    declined(NOT_TAKEN);

    assert!(switch(seller, true).0.contains("200"));
    let (_, placed) = place(&pair);
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, placed["orders"][0]["orderId"].as_str().unwrap())["status"], AWAITING_PAYMENT, "and takes it once it can");
}

/// Terms as a seller's node sends them, for `sats` at [`SATS_PER_ZEC`].
fn terms(sats: u64) -> Value {
    let address = ViewKey::from_seed(&[7u8; 64], ZecNetwork::Testnet).unwrap().address(1);
    let zatoshi = zatoshi_for_sats(sats, SATS_PER_ZEC).unwrap().to_string();
    json!({ "kind": "terms", "rail": "zec", "zcashAddress": address, "amountZatoshi": zatoshi, "satsPerZec": SATS_PER_ZEC, "amountSats": sats, "network": "testnet", "pool": "ironwood", "expiresAt": 2_000_000_000 })
}

#[test]
fn the_buyers_node_checks_zcash_terms_before_showing_them() {
    let good = terms(PRICE);
    let payment = payment_from_terms(&good, ZecNetwork::Testnet).unwrap();
    assert_eq!((&payment["zcashAddress"], &payment["amountZatoshi"], &payment["satsPerZec"]), (&good["zcashAddress"], &good["amountZatoshi"], &good["satsPerZec"]));
    let refused = |change: &dyn Fn(&mut Value)| {
        let mut terms = terms(PRICE);
        change(&mut terms);
        payment_from_terms(&terms, ZecNetwork::Testnet).unwrap_err()
    };
    // A node from before Zcash sales ignores the option and answers in bitcoin.
    assert_eq!(payment_from_terms(&json!({ "kind": "terms", "address": "tb1qexample", "amountSats": PRICE }), ZecNetwork::Testnet).unwrap_err(), NO_ZEC_TERMS);
    let mainnet = ViewKey::from_seed(&[7u8; 64], ZecNetwork::Mainnet).unwrap().address(1);
    for bad in [json!(mainnet), json!("tmNotShielded")] {
        assert!(refused(&|terms| terms["zcashAddress"] = bad.clone()).contains("not a shielded Ironwood address"));
    }
    assert!(refused(&|terms| terms["pool"] = json!("sapling")).contains("not a shielded Ironwood address"));
    assert!(refused(&|terms| terms["network"] = json!("mainnet")).contains("not a shielded Ironwood address"));
    // The ZEC amount must be the sats price at the rate shown, to the zatoshi.
    assert!(refused(&|terms| terms["amountZatoshi"] = json!((zatoshi(&terms["amountZatoshi"]) + 1).to_string())).contains("does not match"));
    assert!(refused(&|terms| terms["satsPerZec"] = json!(SATS_PER_ZEC / 2)).contains("does not match"));
    assert!(refused(&|terms| terms["expiresAt"] = Value::Null).contains("does not match"));
}

#[test]
fn the_buyers_node_declines_bad_zcash_terms_and_ignores_a_stale_update() {
    let shop = Shop::open();
    let buyer = &shop.pair.buyer.node;
    let network = NetworkMode::Testnet4;
    let waiting = || {
        let (_, placed) = place(&shop.pair);
        let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
        buyer.orders().on_network().find(|order| order.id == id).cloned().unwrap()
    };
    let reason = |body: &Value| {
        let mut order = waiting();
        crate::remote_orders::apply_reply(&mut order, body, network);
        assert_eq!(order.status, DECLINED);
        order.remote.unwrap()["reason"].as_str().unwrap().to_string()
    };
    assert_eq!(reason(&json!({ "kind": "terms", "address": "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "amountSats": PRICE })), NO_ZEC_TERMS);
    assert!(reason(&terms(PRICE + 1)).starts_with(&format!("The seller asked {} sats", PRICE + 1)), "a sats price may not rise");

    let mut order = waiting();
    crate::remote_orders::apply_reply(&mut order, &terms(PRICE), network);
    assert_eq!(order.status, AWAITING_PAYMENT);
    let due = zatoshi(&order.payment.as_ref().unwrap()["amountZatoshi"]);
    let told = |seq: u64, status: &str, received: u64| json!({ "seq": seq, "status": status, "receivedZatoshi": received.to_string(), "confirmations": 2, "txids": ["ab"] });
    follow_seller(&mut order, &told(2, PAYMENT_SEEN, due));
    follow_seller(&mut order, &told(1, AWAITING_PAYMENT, due / 2));
    assert_eq!((order.status.as_str(), order.payment.as_ref().unwrap()["receivedZatoshi"].as_str()), (PAYMENT_SEEN, Some(due.to_string().as_str())), "the outbox does not keep replies in order");
    follow_seller(&mut order, &told(3, PAID, due));
    assert_eq!(order.status, PAYMENT_SEEN, "only a paid reply marks it paid");
}

#[test]
fn a_zcash_order_to_another_node_is_for_testnet4_only() {
    let btc = MockChain::new();
    for (network, allowed) in [(NetworkMode::Testnet4, true), (NetworkMode::Mainnet, false)] {
        let (there, here) = (TempDir::new(), TempDir::new());
        // A store on another node whose record takes Zcash.
        let seller = open_node(&there.0, network, &btc, 1, unreachable_escrow());
        open_store(&seller);
        let secret = seller.store_secret(&seller.own_store).expect("unlocked");
        let store = super::with_zec(&seller.own_record().unwrap(), true, &secret).unwrap();
        let node = open_node(&here.0, network, &btc, 1, unreachable_escrow());
        let listings = seller.catalog().listings.clone();
        let mut catalog = node.catalog_mut();
        catalog.stores.push(store);
        catalog.listings.extend(listings);
        drop(catalog);
        assert_eq!(node.zec_ready("Brook and Pine").map_err(|err| err.to_string()), if allowed { Ok(()) } else { Err(OTHER_NODE.to_string()) }, "{network:?}");
        let (_, catalog) = request(&node, "GET", "/catalog", "");
        let offered = catalog["listings"].as_array().unwrap().iter().any(|listing| listing["seller"] == "Brook and Pine" && listing["acceptsZec"] == true);
        assert_eq!(offered, allowed, "{network:?}");
    }
}
