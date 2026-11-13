//! A Monero sale between two nodes, on a mock daemon. Only the seller's node has a chain.

use std::sync::Arc;

use openbazaar_xmr::amount::piconero_for_sats;
use openbazaar_xmr::mock::MockChain as XmrChainMock;
use openbazaar_xmr::{address_for, SpendKey, XmrNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

use super::sale::{follow_seller, payment_from_terms, NotReady, NOT_TAKEN, NO_XMR_TERMS};
use crate::harness::{order, request, ship_to};
use crate::multinode_seller_tests::Pair;
use crate::network::NetworkMode;
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, DECLINED, PAID, PAYMENT_SEEN, SHIPPED};

const SATS_PER_XMR: u64 = 300_000;
/// One of the listing (25,000 sats) and its shipping.
const PRICE: u64 = 28_000;
const NET: XmrNetwork = XmrNetwork::Stagenet;

struct Shop {
    pair: Pair,
    xmr: Arc<XmrChainMock>,
}

impl Shop {
    /// A seller whose own store takes Monero, and a buyer with no daemon that follows it.
    fn open() -> Self {
        let (pair, xmr) = (Pair::new(), Arc::new(XmrChainMock::new(NET)));
        let seller = &pair.seller.node;
        seller.set_xmr_chain(xmr.clone()).unwrap();
        seller.xmr.rate.set_for_tests(Some(SATS_PER_XMR));
        seller.xmr_sync_once().unwrap();
        let own = seller.own_record().expect("published").name;
        let (status, stores) = request(
            seller,
            "PUT",
            "/xmr/stores",
            &json!({ "seller": own, "accept": true }).to_string(),
        );
        assert!(status.contains("200"), "{status} {stores}");
        pair.buyer.node.replicate_once();
        Self { pair, xmr }
    }

    /// Check out one of the listing in Monero on the buyer's node, and carry the mail both ways. Returns the order.
    fn order(&self) -> Value {
        let (status, placed) = place(&self.pair);
        assert!(status.contains("200"), "{status} {placed}");
        assert_eq!(placed["orders"][0]["status"], AWAITING_SELLER);
        self.pair.deliver();
        order(
            &self.pair.buyer.node,
            placed["orders"][0]["orderId"].as_str().unwrap(),
        )
    }

    /// One round of the seller's Monero wallet, then the mail. Returns the seller's sale and the buyer's order.
    fn round(&self, id: &str) -> (Value, Value) {
        self.pair.seller.node.xmr_sync_once().unwrap();
        self.pair.deliver();
        (
            self.pair.sales().remove(0),
            order(&self.pair.buyer.node, id),
        )
    }
}

fn place(pair: &Pair) -> (String, Value) {
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    let listing = catalog["listings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listing| listing["seller"] == "Brook and Pine")
        .cloned()
        .unwrap();
    assert_eq!(
        (&listing["acceptsXmr"], &catalog["xmr"]["enabled"]),
        (&json!(true), &json!(false)),
        "offered with no daemon on the buyer's node"
    );
    let package = json!({ "settlement": "direct", "rail": "xmr", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] });
    request(
        &pair.buyer.node,
        "POST",
        "/checkout",
        &json!({ "shipTo": ship_to(), "packages": [package] }).to_string(),
    )
}

fn piconero(value: &Value) -> u64 {
    value.as_str().unwrap().parse().unwrap()
}

/// Status, piconero received and confirmations: the same on the seller's sale and the buyer's order.
fn progress(of: &Value) -> (&str, u64, u64) {
    (
        of["status"].as_str().unwrap(),
        piconero(&of["payment"]["receivedPiconero"]),
        of["confirmations"].as_u64().unwrap(),
    )
}

#[test]
fn a_monero_sale_is_priced_by_the_seller_and_paid_on_both_nodes() {
    let shop = Shop::open();
    let placed = shop.order();
    let id = placed["id"].as_str().unwrap();
    let due = piconero_for_sats(PRICE, SATS_PER_XMR).unwrap();

    let payment = &placed["payment"];
    assert_eq!(
        (&placed["status"], &placed["totalSats"]),
        (&json!(AWAITING_PAYMENT), &json!(PRICE)),
        "{placed}"
    );
    assert_eq!(
        (&payment["rail"], &payment["network"]),
        (&json!("xmr"), &json!("stagenet"))
    );
    assert_eq!(
        (
            &payment["satsPerXmr"],
            &payment["priceSats"],
            piconero(&payment["amountPiconero"])
        ),
        (&json!(SATS_PER_XMR), &json!(PRICE), due)
    );
    assert!(
        payment["address"].is_null(),
        "no bitcoin address: {payment}"
    );
    assert_eq!(
        payment["watched"],
        json!([]),
        "the buyer's node does not watch the subaddress"
    );
    let sale = shop.pair.sales().remove(0);
    let sale_id = sale["id"].as_str().unwrap().to_string();
    assert_eq!(
        (&sale["status"], &sale["totalSats"], &sale["address"]),
        (&json!(AWAITING_PAYMENT), &json!(PRICE), &Value::Null)
    );
    let address = payment["subaddress"].as_str().unwrap();
    {
        let slot = shop.pair.seller.node.xmr.buyer();
        let index = sale["payment"]["subaddressIndex"].as_u64().unwrap() as u32;
        assert_eq!(
            slot.wallet
                .as_ref()
                .unwrap()
                .keys()
                .subaddress(NET, index)
                .to_string(),
            address,
            "of the own store's wallet"
        );
    }

    let to = address_for(address, NET).unwrap();
    shop.xmr.mine_to(&to, due / 2);
    shop.pair.mine();
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (AWAITING_PAYMENT, due / 2, 1));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert_eq!(
        piconero(&bought["payment"]["remainingPiconero"]),
        due - due / 2
    );

    shop.xmr.mine_to(&to, due - due / 2);
    shop.xmr.mine(CONFIRMATIONS - 2);
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (PAYMENT_SEEN, due, CONFIRMATIONS - 1));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert!(bought["remote"]["sellerSawPayment"].is_null());

    shop.xmr.mine(1);
    let (sale, bought) = shop.round(id);
    assert_eq!(progress(&sale), (PAID, due, CONFIRMATIONS));
    assert_eq!(progress(&bought), progress(&sale), "{bought}");
    assert_eq!(
        (
            &bought["remote"]["sellerSawPayment"],
            bought["txids"].as_array().unwrap().len()
        ),
        (&json!(true), 2)
    );

    let (status, shipped) = request(
        &shop.pair.seller.node,
        "POST",
        &format!("/sales/{sale_id}/fulfill"),
        &json!({ "carrier": "USPS" }).to_string(),
    );
    assert!(
        status.contains("200") && shipped["status"] == SHIPPED,
        "{status} {shipped}"
    );
    shop.pair.deliver();
    assert_eq!(order(&shop.pair.buyer.node, id)["status"], SHIPPED);
}

#[test]
fn the_seller_declines_a_monero_order_it_cannot_take_and_says_why() {
    let pair = Pair::new();
    let seller = &pair.seller.node;
    let secret = seller.store_secret(&seller.own_store).expect("unlocked");
    let rails = crate::own_store::Rails {
        xmr: true,
        ..crate::own_store::Rails::default()
    };
    seller
        .publish_own(openbazaar_protocol::ContentRating::General, rails, &secret)
        .unwrap();
    pair.buyer.node.replicate_once();
    let declined = |why: &str| {
        let (status, placed) = place(&pair);
        assert!(status.contains("200"), "{status} {placed}");
        pair.deliver();
        let order = order(
            &pair.buyer.node,
            placed["orders"][0]["orderId"].as_str().unwrap(),
        );
        assert_eq!(
            (&order["status"], &order["remote"]["reason"]),
            (&json!(DECLINED), &json!(why)),
            "{order}"
        );
        assert!(
            order["payment"].is_null() && pair.sales().is_empty(),
            "nothing to pay, and no sale was opened"
        );
    };
    declined(NotReady::Off.buyer());
    let xmr = Arc::new(XmrChainMock::new(NET));
    seller.set_xmr_chain(xmr).unwrap();
    declined(NotReady::NoPrice.buyer());
    seller.xmr.rate.set_for_tests(Some(SATS_PER_XMR));
    let own = seller.own_record().expect("published").name;
    assert!(request(
        seller,
        "PUT",
        "/xmr/stores",
        &json!({ "seller": own, "accept": false }).to_string()
    )
    .0
    .contains("200"));
    declined(NOT_TAKEN);

    assert!(request(
        seller,
        "PUT",
        "/xmr/stores",
        &json!({ "seller": own, "accept": true }).to_string()
    )
    .0
    .contains("200"));
    seller.xmr_sync_once().unwrap();
    let (_, placed) = place(&pair);
    pair.deliver();
    assert_eq!(
        order(
            &pair.buyer.node,
            placed["orders"][0]["orderId"].as_str().unwrap()
        )["status"],
        AWAITING_PAYMENT,
        "and takes it once it can"
    );
}

fn keys() -> openbazaar_xmr::ViewKeys {
    SpendKey::from_seed_secret(&[7u8; 32]).view_keys()
}

/// Terms as a seller's node sends them, for `sats` at [`SATS_PER_XMR`].
fn terms(sats: u64) -> Value {
    let address = keys().subaddress(NET, 1).to_string();
    let piconero = piconero_for_sats(sats, SATS_PER_XMR).unwrap().to_string();
    json!({ "kind": "terms", "rail": "xmr", "subaddress": address, "amountPiconero": piconero, "satsPerXmr": SATS_PER_XMR, "amountSats": sats, "network": "stagenet" })
}

#[test]
fn the_buyers_node_checks_monero_terms_before_showing_them() {
    let good = terms(PRICE);
    let payment = payment_from_terms(&good, NET).unwrap();
    assert_eq!(
        (
            &payment["subaddress"],
            &payment["amountPiconero"],
            &payment["satsPerXmr"]
        ),
        (
            &good["subaddress"],
            &good["amountPiconero"],
            &good["satsPerXmr"]
        )
    );
    let refused = |change: &dyn Fn(&mut Value)| {
        let mut terms = terms(PRICE);
        change(&mut terms);
        payment_from_terms(&terms, NET).unwrap_err()
    };
    assert_eq!(
        payment_from_terms(
            &json!({ "kind": "terms", "address": "tb1qexample", "amountSats": PRICE }),
            NET
        )
        .unwrap_err(),
        NO_XMR_TERMS
    );
    let mainnet = keys().subaddress(XmrNetwork::Mainnet, 1).to_string();
    let primary = keys().primary_address(NET).to_string();
    for bad in [json!(mainnet), json!(primary)] {
        assert!(refused(&|terms| terms["subaddress"] = bad.clone())
            .contains("not a subaddress on this network"));
    }
    assert!(refused(&|terms| terms["network"] = json!("mainnet"))
        .contains("not a subaddress on this network"));
    assert!(refused(&|terms| terms["amountPiconero"] =
        json!((piconero(&terms["amountPiconero"]) + 1).to_string()))
    .contains("does not match"));
    assert!(
        refused(&|terms| terms["satsPerXmr"] = json!(SATS_PER_XMR / 2)).contains("does not match")
    );
}

#[test]
fn the_buyers_node_declines_bad_monero_terms_and_ignores_a_stale_update() {
    let shop = Shop::open();
    let buyer = &shop.pair.buyer.node;
    let network = NetworkMode::Testnet4;
    let waiting = || {
        let (_, placed) = place(&shop.pair);
        let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
        buyer
            .orders()
            .on_network()
            .find(|order| order.id == id)
            .cloned()
            .unwrap()
    };
    let reason = |body: &Value| {
        let mut order = waiting();
        crate::remote_orders::apply_reply(&mut order, body, network);
        assert_eq!(order.status, DECLINED);
        order.remote.unwrap()["reason"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        reason(
            &json!({ "kind": "terms", "address": "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "amountSats": PRICE })
        ),
        NO_XMR_TERMS
    );
    assert!(
        reason(&terms(PRICE + 1)).starts_with(&format!("The seller asked {} sats", PRICE + 1)),
        "a sats price may not rise"
    );

    let mut order = waiting();
    crate::remote_orders::apply_reply(&mut order, &terms(PRICE), network);
    assert_eq!(order.status, AWAITING_PAYMENT);
    let due = piconero(&order.payment.as_ref().unwrap()["amountPiconero"]);
    let told = |seq: u64, status: &str, received: u64| json!({ "seq": seq, "status": status, "receivedPiconero": received.to_string(), "confirmations": 2, "txids": ["ab"] });
    follow_seller(&mut order, &told(2, PAYMENT_SEEN, due));
    follow_seller(&mut order, &told(1, AWAITING_PAYMENT, due / 2));
    assert_eq!(
        (
            order.status.as_str(),
            order.payment.as_ref().unwrap()["receivedPiconero"].as_str()
        ),
        (PAYMENT_SEEN, Some(due.to_string().as_str())),
        "the outbox does not keep replies in order"
    );
    follow_seller(&mut order, &told(3, PAID, due));
    assert_eq!(
        order.status, PAYMENT_SEEN,
        "only a paid reply marks it paid"
    );
}
