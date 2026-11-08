//! A bid letter is listed for the seller, and the answer is a store-signed
//! offer bound to the conversation key that sent it.

use openbazaar_protocol::pricing::{quote, At, Delivery, Line};
use openbazaar_protocol::{Bid, ListingId, SignedOffer, VariantId};
use serde_json::{json, Value};
use tormail::{open_reply, seal_message, DropId, ReplyDelivery, ReplyKeypair, SenderKey};

use crate::escrow_mail::{hex, unhex};
use crate::harness::{open_node, request, unlock, MockChain, TempDir};
use crate::http::dispatch;
use crate::network::NetworkMode;
use crate::seller_store_tests::{open_store, png, upload};
use crate::vacation::AWAY_MESSAGE;
use crate::{EscrowConfig, Node};

fn post_mail(node: &Node, bytes: &[u8]) -> String {
    let mut raw = format!(
        "POST /mail HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    raw.extend_from_slice(bytes);
    let response = String::from_utf8(dispatch(node, &raw)).unwrap();
    response.lines().next().unwrap().to_string()
}

fn mail_key(node: &Node) -> tormail::StoreKey {
    let catalog = node.catalog();
    let store = catalog.stores.iter().find(|store| store.id == node.own_store).unwrap();
    crate::stores::mailbox_key(store).unwrap()
}

struct Bidder {
    sender: SenderKey,
    reply: ReplyKeypair,
    drop: DropId,
}

impl Bidder {
    fn new() -> Self {
        Self { sender: SenderKey::generate(), reply: ReplyKeypair::generate(), drop: DropId::generate() }
    }

    fn letter(&self, node: &Node, bid: &Bid) -> Vec<u8> {
        let body = json!({
            "v": 1,
            "kind": "bid",
            "bid": hex(&bid.encode().unwrap()),
            "replyUrl": "http://127.0.0.1:9",
            "replyKey": hex(self.reply.public_key().as_bytes()),
            "drop": hex(&self.drop.0),
        });
        seal_message(&serde_json::to_vec(&body).unwrap(), &mail_key(node), &self.sender).unwrap().to_bytes()
    }

    fn replies(&self, node: &Node) -> Vec<Value> {
        node.order_outbox
            .queued()
            .into_iter()
            .filter_map(|(_, frame)| {
                let (_, sealed) = ReplyDelivery::from_frame(&frame).ok()?;
                let opened = open_reply(&sealed, self.reply.secret()).ok()?;
                serde_json::from_slice(&opened.body).ok()
            })
            .collect()
    }
}

fn bid_for(listing_id: &str, amount: u64) -> Bid {
    Bid {
        listing_id: ListingId(crate::escrow_mail::parse_id(listing_id).unwrap()),
        variant_id: VariantId::NONE,
        quantity: 1,
        amount,
        currency: "USD".into(),
    }
}

fn digital(node: &Node, offers: bool) -> Value {
    let (_, cover) = upload(node, &png(4));
    let body = json!({
        "title": "Glaze recipes",
        "description": "Forty cone-6 glazes.",
        "category": "Books",
        "kind": "digital",
        "stock": 20,
        "images": [cover["hash"]],
        "currency": "USD",
        "price": 2_500,
        "shipping": 0,
        "contentRating": "general",
        "commerce": { "condition": "new", "acceptsOffers": offers },
    });
    let (status, listing) = request(node, "POST", "/store/listings", &body.to_string());
    assert!(status.contains("200"), "{status} {listing}");
    listing
}

#[test]
fn a_bid_is_answered_with_an_offer_only_that_buyer_can_check() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let bidder = Bidder::new();
    let (listing_id, bid_id) = {
        let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
        node.fix_rates(&[("USD", 100_000.0)]);
        let _ = open_store(&node);
        let listing = digital(&node, true);
        let listing_id = listing["listingId"].as_str().unwrap().to_string();
        let bid = bid_for(&listing_id, 2_000);
        let status = post_mail(&node, &bidder.letter(&node, &bid));
        assert!(status.contains("200"), "{status}");
        post_mail(&node, &bidder.letter(&node, &bid));
        let (status, listed) = request(&node, "GET", "/offers", "");
        assert!(status.contains("200"), "{status} {listed}");
        let offers = listed["offers"].as_array().unwrap();
        assert_eq!(offers.len(), 1, "the same bid is one row");
        let row = &offers[0];
        for absent in ["buyer", "buyerKey", "replyUrl", "replyKey", "drop"] {
            assert!(row.get(absent).is_none(), "{absent} is not part of the offer list");
        }
        assert_eq!(row["listingId"], listing_id);
        assert_eq!(row["variantId"], "0000000000000000");
        assert_eq!(row["quantity"], 1);
        assert_eq!(row["amount"], 2_000);
        assert_eq!(row["currency"], "USD");
        (listing_id, row["id"].as_str().unwrap().to_string())
    };

    let node = Node::open(&dir.0, crate::load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
    assert_eq!(request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().len(), 1, "a bid survives a restart");
    let (status, _) = request(&node, "POST", &format!("/offers/{bid_id}/answer"), r#"{"expiresAt":9999999999}"#);
    assert!(status.contains("423"), "a locked wallet cannot sign an offer: {status}");
    assert_eq!(request(&node, "GET", "/offers", "").1["offers"][0]["id"], bid_id);
    unlock(&node);
    let expires_at = crate::escrow_outbox::now_secs() + 3_600;
    let answer = json!({
        "amount": 1_800,
        "expiresAt": expires_at,
        "buyerKey": "00".repeat(32),
    });
    let (status, answered) = request(&node, "POST", &format!("/offers/{bid_id}/answer"), &answer.to_string());
    assert!(status.contains("200"), "{status} {answered}");
    assert_eq!(answered["counter"], true);
    assert_eq!(answered["amount"], 1_800);
    let signed = SignedOffer::decode(&unhex(answered["offer"].as_str().unwrap()).unwrap()).unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.terms.buyer_key, bidder.sender.id().0, "the offer binds to the conversation key, not a key the answer names");
    assert_eq!(signed.terms.amount, 1_800);
    assert!(signed.terms.counter);
    assert_eq!(signed.terms.expires_at, expires_at);
    assert_eq!(hex(&signed.terms.offer_id), bid_id);
    let replies = bidder.replies(&node);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["kind"], "offer");
    assert_eq!(replies[0]["offer"], answered["offer"]);
    assert!(request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().is_empty());
    let (status, _) = request(&node, "POST", &format!("/offers/{bid_id}/answer"), &answer.to_string());
    assert!(status.contains("404"), "{status}");
    let again = post_mail(&node, &bidder.letter(&node, &bid_for(&listing_id, 2_000)));
    assert!(again.contains("200"), "{again}");
    assert!(request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().is_empty(), "an answered bid is not listed again");

    let listing = node.own_listings().by_id(&ListingId(crate::escrow_mail::parse_id(&listing_id).unwrap())).cloned().unwrap();
    let line = Line { listing: &listing, variant: VariantId::NONE, quantity: 1, delivery: Delivery::Digital, offer: Some(&signed) };
    let buyer = bidder.sender.id().0;
    let priced = quote(std::slice::from_ref(&line), None, At { today: 20261108, now: expires_at - 1, buyer_key: Some(&buyer), country: None }).unwrap();
    assert_eq!(priced.total, 1_800);
    let other = [9u8; 32];
    let wrong = quote(std::slice::from_ref(&line), None, At { today: 20261108, now: expires_at - 1, buyer_key: Some(&other), country: None });
    assert!(wrong.is_err(), "another conversation key cannot use the offer");
}

#[test]
fn a_bid_is_declined_when_the_listing_does_not_take_offers_or_the_store_is_away() {
    let dir = TempDir::new();
    let chain = MockChain::new();
    let node = open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, EscrowConfig::default());
    node.fix_rates(&[("USD", 100_000.0)]);
    let _ = open_store(&node);
    let closed = digital(&node, false);
    let bidder = Bidder::new();
    let status = post_mail(&node, &bidder.letter(&node, &bid_for(closed["listingId"].as_str().unwrap(), 1_000)));
    assert!(status.contains("200"), "{status}");
    let replies = bidder.replies(&node);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["kind"], "declined");
    assert_eq!(replies[0]["reason"], "This listing is not taking offers.");
    assert!(request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().is_empty());

    let listing = digital(&node, true);
    let (status, _) = request(&node, "PUT", "/store/vacation", r#"{"away":true}"#);
    assert!(status.contains("200"), "{status}");
    let away = bid_for(listing["listingId"].as_str().unwrap(), 1_000);
    assert!(post_mail(&node, &bidder.letter(&node, &away)).contains("200"));
    let replies = bidder.replies(&node);
    assert!(replies.iter().any(|reply| reply["reason"] == AWAY_MESSAGE));
    assert!(request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().is_empty());
    let (status, _) = request(&node, "PUT", "/store/vacation", r#"{"away":false}"#);
    assert!(status.contains("200"), "{status}");
    assert!(post_mail(&node, &bidder.letter(&node, &away)).contains("200"));
    let higher = bid_for(listing["listingId"].as_str().unwrap(), 1_500);
    assert!(post_mail(&node, &bidder.letter(&node, &higher)).contains("200"));
    let offers = request(&node, "GET", "/offers", "").1["offers"].as_array().unwrap().clone();
    assert_eq!(offers.len(), 2, "a different amount is a new bid");
    assert_eq!(offers.iter().filter(|row| row["amount"] == 1_000).count(), 1);
    assert_eq!(offers.iter().filter(|row| row["amount"] == 1_500).count(), 1);
}
