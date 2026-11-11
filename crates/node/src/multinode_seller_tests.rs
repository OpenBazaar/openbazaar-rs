//! A seller and a buyer on two nodes, end to end: the seller opens a store
//! and lists, the buyer follows it, orders, and pays on the shared chain,
//! the seller's node takes the order over the order mail, sees the payment,
//! ships, and the buyer confirms receipt.

use serde_json::{json, Value};

use crate::harness::{fund_node_wallet, order, pay, request, ship_to, MockChain, Peer};
use crate::seller_store_tests::{open_store, png, upload};
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, COMPLETED, DECLINED, PAID, PAYMENT_SEEN, SHIPPED};
use crate::EscrowConfig;

fn peer(chain: &MockChain) -> Peer {
    Peer::spawn(chain, |url| EscrowConfig { mailbox_url: Some(url), ..EscrowConfig::default() })
}

struct Pair {
    chain: MockChain,
    seller: Peer,
    buyer: Peer,
    listing: Value,
}

impl Pair {
    /// A seller with one listing, and a buyer that follows it.
    fn new() -> Self {
        let chain = MockChain::new();
        let seller = peer(&chain);
        let buyer = peer(&chain);
        let listing = open_store(&seller.node);
        seller.node.record_finished_escrow_sale().expect("an established store has one finished escrow");
        let (status, follows) = request(&buyer.node, "POST", "/follows", &json!({ "url": seller.url }).to_string());
        assert!(status.contains("200"), "{status} {follows}");
        assert_eq!(follows["follows"][0]["name"], "Brook and Pine", "{follows}");
        Self { chain, seller, buyer, listing }
    }

    fn hash(&self) -> &str {
        self.listing["hash"].as_str().unwrap()
    }

    /// Check out the listing on the buyer's node. Returns the order.
    fn checkout(&self, quantity: u32) -> Value {
        let body = json!({
            "shipTo": ship_to(),
            "packages": [{ "settlement": "direct", "lines": [{ "listingHash": self.hash(), "quantity": quantity }] }],
        });
        let (status, value) = request(&self.buyer.node, "POST", "/checkout", &body.to_string());
        assert!(status.contains("200"), "{status} {value}");
        value["orders"][0].clone()
    }

    /// Send every queued letter and reply, both ways.
    fn deliver(&self) {
        for _ in 0..3 {
            self.buyer.node.flush_order_mail(true);
            self.seller.node.flush_order_mail(true);
        }
    }

    fn sales(&self) -> Vec<Value> {
        let (_, sales) = request(&self.seller.node, "GET", "/sales", "");
        sales["sales"].as_array().unwrap().clone()
    }

    fn mine(&self) {
        self.chain.mine();
        self.seller.node.watch_once().unwrap();
        self.buyer.node.watch_once().unwrap();
    }
}

#[test]
fn a_sale_goes_from_listing_to_completed_across_two_nodes() {
    let pair = Pair::new();
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    let copy = catalog["listings"].as_array().unwrap().iter().find(|l| l["hash"] == pair.hash()).cloned().unwrap();
    assert_eq!(copy["listingId"], pair.listing["listingId"]);
    let image = crate::stores::unhex32(copy["images"][0].as_str().unwrap()).unwrap();
    assert_eq!(pair.buyer.node.media.get(&image).unwrap().1, png(2), "the buyer's node fetched the image");

    let placed = pair.checkout(2);
    let id = placed["orderId"].as_str().unwrap().to_string();
    assert_eq!(placed["status"], AWAITING_SELLER);
    assert_eq!(placed["totalSats"], 2 * 25_000 + 3_000);
    assert!(placed.get("payment").is_none(), "only the seller names the address");
    assert_eq!(order(&pair.buyer.node, &id)["remote"]["delivered"], true);

    let sales = pair.sales();
    assert_eq!(sales.len(), 1, "the seller's node took the order");
    let sale = &sales[0];
    let sale_id = sale["id"].as_str().unwrap().to_string();
    assert_eq!((sale["status"].as_str(), sale["totalSats"].as_u64()), (Some(AWAITING_PAYMENT), Some(53_000)));
    assert!(sale["shipTo"].as_str().unwrap().contains("14 Larch Lane"), "{sale}");
    assert!(sale.get("replyUrl").is_none(), "the reply route stays inside the node");
    assert!(pair.seller.node.buyer_wallet().wallet.is_mine(
        crate::harness::address_any(sale["address"].as_str().unwrap()).script_pubkey()
    ));

    pair.deliver();
    let terms = order(&pair.buyer.node, &id);
    assert_eq!(terms["status"], AWAITING_PAYMENT, "{terms}");
    assert_eq!(terms["payment"]["address"], sale["address"]);
    assert_eq!(terms["totalSats"], 53_000);

    fund_node_wallet(&pair.buyer.node, &pair.chain, 200_000);
    let (status, paid) = pay(&pair.buyer.node, &id, "{}");
    assert!(status.contains("202"), "{status} {paid}");
    pair.deliver();
    let told = &pair.sales()[0];
    assert_eq!(told["status"], AWAITING_PAYMENT, "the seller has not watched the chain yet");
    assert_eq!(told["sentTxid"], order(&pair.buyer.node, &id)["payTxid"], "but the buyer said what it sent");
    pair.seller.node.watch_once().unwrap();
    assert_eq!(pair.sales()[0]["status"], PAYMENT_SEEN);
    pair.mine();
    assert_eq!(pair.sales()[0]["status"], PAID);
    pair.deliver();
    let seen = order(&pair.buyer.node, &id);
    assert_eq!(seen["status"], PAID);
    assert_eq!(seen["remote"]["sellerSawPayment"], true);

    let (status, refused) = request(&pair.buyer.node, "POST", &format!("/orders/{id}/complete"), "");
    assert!(status.contains("400"), "not before it ships: {status} {refused}");
    let fulfill = json!({ "carrier": "USPS", "tracking": "9400 1000 0000 0000 0000 00" }).to_string();
    let (status, shipped) = request(&pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), &fulfill);
    assert!(status.contains("200"), "{status} {shipped}");
    assert_eq!(shipped["status"], SHIPPED);
    pair.deliver();
    let arriving = order(&pair.buyer.node, &id);
    assert_eq!(arriving["status"], SHIPPED);
    assert_eq!(arriving["remote"]["fulfillment"]["tracking"], "9400 1000 0000 0000 0000 00");

    let (status, done) = request(&pair.buyer.node, "POST", &format!("/orders/{id}/complete"), "");
    assert!(status.contains("200"), "{status} {done}");
    assert_eq!(done["status"], COMPLETED);
    assert_eq!(pair.sales()[0]["status"], COMPLETED);
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, &id)["remote"]["sellerCompleted"], true);
}

#[test]
fn an_offline_seller_gets_the_order_when_it_is_back_and_the_buyer_keeps_its_copy() {
    let mut pair = Pair::new();
    pair.seller.stop();
    let chain = pair.chain.clone();
    pair.buyer.restart(&chain, true);
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    assert!(
        catalog["listings"].as_array().unwrap().iter().any(|l| l["hash"] == pair.hash()),
        "the copy survives a restart while the seller is away"
    );
    let placed = pair.checkout(1);
    let id = placed["orderId"].as_str().unwrap().to_string();
    let waiting = order(&pair.buyer.node, &id);
    assert_eq!(waiting["remote"]["delivered"], false);
    assert!(waiting["remote"]["error"].as_str().unwrap().contains("offline"), "{waiting}");

    pair.buyer.restart(&chain, true);
    pair.seller.start();
    pair.deliver();
    assert_eq!(pair.sales().len(), 1, "the outbox kept the letter across the buyer's restart");
    assert_eq!(order(&pair.buyer.node, &id)["status"], AWAITING_PAYMENT);
}

#[test]
fn a_locked_seller_still_takes_orders_and_watches_payments() {
    let mut pair = Pair::new();
    let chain = pair.chain.clone();
    pair.seller.restart(&chain, false);
    assert_ne!(pair.seller.node.lock.state(), crate::wallet_lock::State::Unlocked);
    let id = pair.checkout(1)["orderId"].as_str().unwrap().to_string();
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, &id)["status"], AWAITING_PAYMENT);
    fund_node_wallet(&pair.buyer.node, &pair.chain, 100_000);
    pay(&pair.buyer.node, &id, "{}");
    pair.mine();
    assert_eq!(pair.sales()[0]["status"], PAID);
    let sale_id = pair.sales()[0]["id"].as_str().unwrap().to_string();
    let (status, _) = request(&pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), r#"{"tracking":"1Z"}"#);
    assert!(status.contains("200"), "shipping needs no spend key: {status}");
}

#[test]
fn sales_and_orders_survive_restarts_mid_flow() {
    let mut pair = Pair::new();
    let chain = pair.chain.clone();
    let id = pair.checkout(1)["orderId"].as_str().unwrap().to_string();
    pair.seller.restart(&chain, true);
    pair.deliver();
    pair.buyer.restart(&chain, true);
    assert_eq!(order(&pair.buyer.node, &id)["status"], AWAITING_PAYMENT);
    fund_node_wallet(&pair.buyer.node, &pair.chain, 100_000);
    pay(&pair.buyer.node, &id, "{}");
    pair.mine();
    pair.seller.restart(&chain, true);
    let sale_id = pair.sales()[0]["id"].as_str().unwrap().to_string();
    request(&pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), r#"{"tracking":"1Z"}"#);
    pair.seller.restart(&chain, true);
    pair.deliver();
    assert_eq!(order(&pair.buyer.node, &id)["status"], SHIPPED, "the queued reply survived the seller's restart");
}

#[test]
fn a_digital_sale_delivers_its_content_in_the_reply() {
    let pair = Pair::new();
    let (_, cover) = upload(&pair.seller.node, &png(7));
    let ebook = json!({
        "title": "Glaze recipes (PDF)",
        "description": "Forty cone-6 glazes.",
        "category": "Books",
        "kind": "digital",
        "stock": 100,
        "images": [cover["hash"]],
        "price": 9_000,
    });
    let (status, listing) = request(&pair.seller.node, "POST", "/store/listings", &ebook.to_string());
    assert!(status.contains("200"), "{status} {listing}");
    pair.buyer.node.replicate_once();
    let body = json!({
        "deliveryNote": "ada@example.com",
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": listing["hash"], "quantity": 1 }] }],
    });
    let (status, placed) = request(&pair.buyer.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {placed}");
    let id = placed["orders"][0]["orderId"].as_str().unwrap().to_string();
    assert_eq!(placed["orders"][0]["totalSats"], 9_000, "no shipping on a digital good");
    pair.deliver();
    fund_node_wallet(&pair.buyer.node, &pair.chain, 100_000);
    pay(&pair.buyer.node, &id, "{}");
    pair.mine();
    let sale = pair.sales()[0].clone();
    assert_eq!(sale["deliveryNote"], "ada@example.com");
    let sale_id = sale["id"].as_str().unwrap();
    let (status, _) = request(&pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), "{}");
    assert!(status.contains("400"), "a digital sale needs its content: {status}");
    let content = json!({ "content": "https://example.com/d/8c1f", "note": "Thanks!" }).to_string();
    request(&pair.seller.node, "POST", &format!("/sales/{sale_id}/fulfill"), &content);
    pair.deliver();
    let got = order(&pair.buyer.node, &id);
    assert_eq!(got["status"], SHIPPED);
    assert_eq!(got["remote"]["fulfillment"]["content"], "https://example.com/d/8c1f");
}

#[test]
fn edits_and_deletes_reach_the_buyer_and_an_old_address_still_orders() {
    let pair = Pair::new();
    let id = pair.listing["listingId"].as_str().unwrap();
    let mut edit = crate::seller_store_tests::mug(pair.listing["images"][0].as_str().unwrap());
    edit["title"] = json!("Hand-thrown mug, large");
    let (_, edited) = request(&pair.seller.node, "PUT", &format!("/store/listings/{id}"), &edit.to_string());
    let placed = pair.checkout(1);
    pair.deliver();
    assert_eq!(pair.sales()[0]["lines"][0]["title"], "Hand-thrown mug, large", "priced against the current version");
    assert_eq!(order(&pair.buyer.node, &placed["orderId"].as_str().unwrap().to_string())["status"], AWAITING_PAYMENT);

    assert_eq!(pair.buyer.node.replicate_once(), 1);
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    let titles: Vec<&str> = catalog["listings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["listingId"] == id)
        .map(|l| l["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Hand-thrown mug, large"]);
    assert_eq!(catalog["listings"].as_array().unwrap().iter().filter(|l| l["hash"] == edited["hash"]).count(), 1);

    request(&pair.seller.node, "DELETE", &format!("/store/listings/{id}"), "");
    pair.buyer.node.replicate_once();
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    assert!(!catalog["listings"].as_array().unwrap().iter().any(|l| l["listingId"] == id));
}

#[test]
fn the_seller_declines_what_it_cannot_sell_and_the_buyer_refuses_inflated_terms() {
    let pair = Pair::new();
    let placed = pair.checkout(6);
    pair.deliver();
    let declined = order(&pair.buyer.node, placed["orderId"].as_str().unwrap());
    assert_eq!(declined["status"], DECLINED, "{declined}");
    assert!(declined["remote"]["reason"].as_str().unwrap().contains("Only 5"), "{declined}");
    assert!(pair.sales().is_empty());

    // The seller raises the price after the buyer's copy was taken.
    let id = pair.listing["listingId"].as_str().unwrap();
    let mut edit = crate::seller_store_tests::mug(pair.listing["images"][0].as_str().unwrap());
    edit["price"] = json!(90_000);
    request(&pair.seller.node, "PUT", &format!("/store/listings/{id}"), &edit.to_string());
    let placed = pair.checkout(1);
    pair.deliver();
    let refused = order(&pair.buyer.node, placed["orderId"].as_str().unwrap());
    assert_eq!(refused["status"], DECLINED, "{refused}");
    assert!(refused["remote"]["reason"].as_str().unwrap().contains("checkout showed"), "{refused}");
    assert!(refused.get("payment").is_none(), "nothing to pay");
}

#[test]
fn mail_from_strangers_is_refused() {
    let pair = Pair::new();
    let post = |path: &str, body: &[u8], origin: bool| {
        let origin = if origin { "Origin: http://127.0.0.1:4173\r\n" } else { "" };
        let raw = [
            format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{origin}Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
            body,
        ]
        .concat();
        String::from_utf8_lossy(&crate::http::dispatch(&pair.seller.node, &raw)).to_string()
    };
    assert!(post("/mail", b"not a letter", false).starts_with("HTTP/1.1 400"));
    assert!(post("/mail", b"x", true).starts_with("HTTP/1.1 403"), "pages may not post mail");
    assert!(post("/mail/reply", b"OBRF", false).starts_with("HTTP/1.1 400"));

    // A complete message for a sale, from a key that is not the buyer's.
    let placed = pair.checkout(1);
    pair.deliver();
    let sale_id = pair.sales()[0]["id"].as_str().unwrap().to_string();
    let store = pair.seller.node.own_record().unwrap();
    let key = crate::stores::mailbox_key(&store).unwrap();
    let body = serde_json::to_vec(&json!({ "kind": "complete", "sale": sale_id })).unwrap();
    let letter = tormail::seal_message(&body, &key, &tormail::SenderKey::generate()).unwrap();
    let response = post("/mail", &letter.to_bytes(), false);
    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    assert_eq!(pair.sales()[0]["status"], AWAITING_PAYMENT);
    assert_eq!(order(&pair.buyer.node, placed["orderId"].as_str().unwrap())["status"], AWAITING_PAYMENT);
}

#[test]
fn a_buyer_cannot_order_from_its_own_store_or_follow_itself() {
    let pair = Pair::new();
    let body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "direct", "lines": [{ "listingHash": pair.hash(), "quantity": 1 }] }],
    });
    let (status, refused) = request(&pair.seller.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("400") && refused["error"].as_str().unwrap().contains("own store"), "{status} {refused}");
    let (status, _) = request(&pair.seller.node, "POST", "/follows", &json!({ "url": pair.seller.url }).to_string());
    assert!(status.contains("400"), "{status}");
    let escrow = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "escrow", "lines": [{ "listingHash": pair.hash(), "quantity": 1 }] }],
    });
    let (status, _) = request(&pair.buyer.node, "POST", "/checkout", &escrow.to_string());
    assert!(status.contains("400"), "own-store listings take direct pay only: {status}");
}

#[test]
fn a_store_on_another_node_takes_no_zcash_yet() {
    let pair = Pair::new();
    let zec = std::sync::Arc::new(openbazaar_zec::mock::MockChain::new(crate::network::NetworkMode::Testnet4.zcash()));
    pair.buyer.node.set_zec_chain(zec).unwrap();
    let body = json!({
        "shipTo": ship_to(),
        "packages": [{ "settlement": "direct", "rail": "zec", "lines": [{ "listingHash": pair.hash(), "quantity": 1 }] }],
    });
    let (status, refused) = request(&pair.buyer.node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("400") && refused["error"] == "This store does not accept Zcash.", "{status} {refused}");
    assert_eq!(pair.buyer.node.orders().on_network().count(), 0, "no letter went to the seller");
    let (_, catalog) = request(&pair.buyer.node, "GET", "/catalog", "");
    let copy = catalog["listings"].as_array().unwrap().iter().find(|l| l["hash"] == pair.hash()).cloned().unwrap();
    assert_eq!(copy["acceptsZec"], false, "the seller's signed record does not set the Zcash bit");
}
