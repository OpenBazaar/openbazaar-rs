//! A verified-purchase review between nodes: the buyer reviews a paid order
//! under its per-order key, the buyer's node publishes it to the seller's
//! node and to the nodes it follows, the seller signs one reply, and the
//! review stays visible on other nodes after the seller leaves it out of
//! its own pool.

use serde_json::{json, Value};

use crate::harness::{fund_node_wallet, order, pay, request, MockChain, Peer};
use crate::multinode_variant_tests::{checkout, deliver, glazed_mug, peer};
use crate::review_publish::MAX_DELAY_SECS;
use crate::watch::PAID;

fn reviews(peer: &Peer, store: &str) -> Value {
    let (status, value) = request(&peer.node, "GET", &format!("/stores/{store}/reviews"), "");
    assert!(status.contains("200"), "{status} {value}");
    value
}

fn follow(peer: &Peer, url: &str) {
    let _ = request(&peer.node, "POST", "/follows", &json!({ "url": url }).to_string());
}

#[test]
fn a_paid_order_gets_one_review_that_the_seller_can_answer_but_not_remove() {
    let chain = MockChain::new();
    let (seller, buyer, witness, newcomer) = (peer(&chain), peer(&chain), peer(&chain), peer(&chain));
    for node in [&seller.node, &buyer.node] {
        node.fix_rates(&[("USD", 100_000.0)]);
    }
    let listing = glazed_mug(&seller);
    let store = seller.node.own_store.to_hex();
    let sea = listing["commerce"]["variants"][0]["id"].as_str().unwrap().to_string();
    follow(&witness, &seller.url);
    follow(&buyer, &seller.url);
    follow(&buyer, &witness.url);

    let placed = checkout(&buyer, json!([{ "listingHash": listing["hash"], "quantity": 1, "variant": sea }]), "");
    let id = placed["orderId"].as_str().unwrap().to_string();
    let second = checkout(&buyer, json!([{ "listingHash": listing["hash"], "quantity": 1, "variant": sea }]), "");
    deliver(&seller, &buyer);
    let early = request(&buyer.node, "POST", &format!("/orders/{id}/review"), &json!({ "rating": 5 }).to_string());
    assert!(early.0.contains("400") && early.1["error"].as_str().unwrap().contains("paid"), "{early:?}");

    fund_node_wallet(&buyer.node, &chain, 200_000);
    assert!(pay(&buyer.node, &id, "{}").0.contains("202"));
    chain.mine();
    seller.node.watch_once().unwrap();
    buyer.node.watch_once().unwrap();
    deliver(&seller, &buyer);
    assert_eq!(order(&buyer.node, &id)["status"], PAID);

    let body = json!({ "rating": 4, "text": "Lovely glaze, one tiny chip on the base." });
    let (status, written) = request(&buyer.node, "POST", &format!("/orders/{id}/review"), &body.to_string());
    assert!(status.contains("200"), "{status} {written}");
    assert!(written["remote"]["review"]["sendAfter"].as_u64().is_some());
    let later = crate::escrow_outbox::now_secs() + MAX_DELAY_SECS + 1;
    assert!(buyer.node.publish_reviews(later) >= 1);
    let published = order(&buyer.node, &id)["remote"]["review"].clone();
    assert_eq!(published["published"], true, "{published}");
    assert_eq!(published["sentTo"].as_array().map(Vec::len), Some(2), "sent to the seller's node and the followed witness: {published}");

    let at_seller = reviews(&seller, &store);
    assert_eq!((at_seller["count"].as_u64(), at_seller["average"].as_f64()), (Some(1), Some(4.0)), "{at_seller}");
    let review = at_seller["reviews"][0].clone();
    assert_eq!(review["text"], "Lovely glaze, one tiny chip on the base.");
    assert_eq!(review["listingTitle"], listing["title"]);
    assert_eq!(review["paidMonth"].as_u64(), Some(u64::from(crate::own_store::today() / 100)), "the receipt names a month, not a day");
    let fields: Vec<&String> = review.as_object().unwrap().keys().collect();
    assert_eq!(fields, ["id", "listingId", "listingTitle", "paidMonth", "rating", "reply", "text", "writtenOn"], "nothing names the buyer");
    assert_eq!(reviews(&witness, &store)["count"], 1, "the witness keeps the buyer's push");

    let again = request(&buyer.node, "POST", &format!("/orders/{id}/review"), &json!({ "rating": 1 }).to_string());
    assert!(again.0.contains("400"), "one review per order, changed at most once a day: {again:?}");

    let review_id = review["id"].as_str().unwrap();
    let reply = json!({ "text": "Sorry about the chip. Message me and I will send a replacement." });
    let (status, answered) = request(&seller.node, "POST", &format!("/reviews/{review_id}/reply"), &reply.to_string());
    assert!(status.contains("200"), "{status} {answered}");
    let other = request(&witness.node, "POST", &format!("/reviews/{review_id}/reply"), &reply.to_string());
    assert!(other.0.contains("400"), "only the reviewed store replies: {other:?}");
    witness.node.replicate_once();
    assert_eq!(reviews(&witness, &store)["reviews"][0]["reply"]["text"], reply["text"]);

    seller.node.reviews().forget_store(&seller.node.own_store);
    assert_eq!(reviews(&seller, &store)["count"], 0, "the seller left the review out of its own pool");
    seller.node.replicate_once();
    witness.node.replicate_once();
    buyer.node.replicate_once();
    for (name, peer) in [("witness", &witness), ("buyer", &buyer)] {
        let kept = reviews(peer, &store);
        assert_eq!(kept["count"], 1, "the {name} still has it: {kept}");
        assert_eq!(kept["reviews"][0]["text"], "Lovely glaze, one tiny chip on the base.");
    }
    follow(&newcomer, &seller.url);
    follow(&newcomer, &witness.url);
    newcomer.node.replicate_once();
    let fresh = reviews(&newcomer, &store);
    assert_eq!(fresh["count"], 1, "a node that follows the seller later still finds it through the witness: {fresh}");
    assert_eq!(fresh["reviews"][0]["reply"]["text"], reply["text"]);

    let store_key = crate::stores::unhex32(&store).unwrap();
    let key = |order: &Value| buyer.node.order_review_key(order["checkoutId"].as_str().unwrap(), &store_key).unwrap();
    assert_ne!(key(&order(&buyer.node, &id)), key(&order(&buyer.node, second["orderId"].as_str().unwrap())), "each order reviews under its own key");
}
