//! Writing, publishing and copying reviews.
//!
//! The buyer writes a review on a paid order to another node's store. The
//! node keeps it with the order and signs it, with that order's review key,
//! after a random wait of up to three days, dated the day it is signed.
//! Then it goes into this node's pool and is sent to the seller's node and
//! to every node this one follows, which keep it in theirs. Each
//! replication round also pulls the pools of followed nodes. The seller
//! signs at most one reply per review with the store key.

use std::io::Read;
use std::time::Duration;

use openbazaar_protocol::{Reply, Review, SignedReceipt, SignedReply, SignedReview, StoreId, MAX_REVIEW_TEXT};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::now_secs;
use crate::reviews::{decode_replies, decode_reviews, ReviewPool};
use crate::watch::{COMPLETED, PAID, SHIPPED};
use crate::Node;

pub(crate) const PUBLIC_REVIEWS: &str = "/public/reviews";
/// The longest a signed review waits before it is published.
pub(crate) const MAX_DELAY_SECS: u64 = 3 * 24 * 3600;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 4 * 1024 * 1024;

fn reviewable(status: &str) -> bool {
    status == PAID || status == SHIPPED || status == COMPLETED
}

impl Node {
    pub(crate) fn reviews(&self) -> std::sync::MutexGuard<'_, ReviewPool> {
        self.reviews.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Whether this node carries `store` in its catalog.
    fn carries(&self) -> impl Fn(&StoreId) -> bool {
        let carried: Vec<StoreId> = self.catalog().stores.iter().map(|store| store.id).collect();
        move |id| carried.contains(id)
    }

    /// Merge reviews and replies from another node. Returns how many were new.
    pub(crate) fn take_reviews(&self, value: &Value) -> usize {
        let carries = self.carries();
        self.reviews().merge(decode_reviews(value), decode_replies(value), carries)
    }

    /// `POST /orders/{id}/review`: `{ rating, text, listingId? }`. The review
    /// is signed and published later; see the module notes.
    pub(crate) fn write_review(&self, id: &str, body: &Value) -> Result<Value, String> {
        let order = self.orders().get(id).cloned().ok_or("That order is not on this node.")?;
        let remote = order.remote.as_ref().ok_or("Only an order to another node's store can be reviewed.")?;
        if !reviewable(&order.status) {
            return Err("Review an order once it is paid.".into());
        }
        let published_on = remote["review"]["writtenOn"].as_u64().filter(|_| remote["review"]["published"] == true);
        if published_on.is_some_and(|day| day >= u64::from(crate::own_store::today())) {
            return Err("Your review went out today. You can change it from tomorrow.".into());
        }
        let rating = body["rating"].as_u64().filter(|n| (1..=5).contains(n)).ok_or("Choose 1 to 5 stars.")? as u8;
        let text = body["text"].as_str().unwrap_or("").trim().to_string();
        if text.len() > MAX_REVIEW_TEXT {
            return Err("Keep a review to 1,000 characters.".into());
        }
        let receipt = self.order_receipt(&order, remote, body["listingId"].as_str())?;
        let draft = self.sign_review(&order, receipt.clone(), rating, &text)?;
        let listing_id = draft.listing_id().to_hex();
        let wait = StdRng::from_os_rng().next_u64() % (MAX_DELAY_SECS + 1);
        self.orders()
            .update(id, |order| {
                if let Some(remote) = order.remote.as_mut() {
                    remote["review"] = json!({ "listingId": listing_id, "rating": rating, "text": text, "sendAfter": now_secs() + wait, "published": false });
                }
            })
            .map_err(|err| err.to_string())?;
        Ok(self.orders().get_value(id).unwrap_or_default())
    }

    /// The seller's receipt for `listing` in this order, or for its first listing.
    fn order_receipt(&self, order: &crate::buyer_orders::BuyerOrder, remote: &Value, listing: Option<&str>) -> Result<SignedReceipt, String> {
        let receipts: Vec<SignedReceipt> = remote["receipts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|text| text.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| SignedReceipt::decode(&bytes).ok()))
            .filter(|receipt| receipt.verify().is_ok() && receipt.terms.store_id.to_hex() == order.store_id)
            .collect();
        receipts
            .into_iter()
            .find(|receipt| listing.is_none_or(|id| receipt.terms.listing_id.to_hex() == id))
            .ok_or_else(|| "The seller has not sent a receipt for this order yet. It comes with the seller's confirmation of payment.".to_string())
    }

    fn sign_review(&self, order: &crate::buyer_orders::BuyerOrder, receipt: SignedReceipt, rating: u8, text: &str) -> Result<SignedReview, String> {
        let store = crate::stores::unhex32(&order.store_id).ok_or("The order names no store key.")?;
        let secret = self.order_review_secret(&order.checkout_id, &store)?;
        let written_on = crate::own_store::today().max(receipt.terms.paid_on);
        let review = Review { receipt, rating, text: text.to_string(), written_on };
        SignedReview::sign(review, &secret).map_err(|err| match err {
            openbazaar_protocol::Error::PrivateText { .. } => "Leave contact details, addresses and links out of a review; it is public.".to_string(),
            openbazaar_protocol::Error::BadKey => "This order's review key does not match the seller's receipt.".to_string(),
            other => other.to_string(),
        })
    }

    /// Sign and publish every review whose wait is over at `now`, and send
    /// published reviews to the nodes that have not taken them yet.
    pub(crate) fn publish_reviews(&self, now: u64) -> usize {
        let due: Vec<crate::buyer_orders::BuyerOrder> = self
            .orders()
            .on_network()
            .filter(|order| order.remote.as_ref().is_some_and(|remote| remote["review"].is_object()))
            .cloned()
            .collect();
        let mut changed = 0;
        for order in due {
            let Some(remote) = order.remote.clone() else { continue };
            let review = &remote["review"];
            let mut record = review["record"].as_str().map(str::to_string);
            if review["published"] != true {
                if review["sendAfter"].as_u64().unwrap_or(u64::MAX) > now {
                    continue;
                }
                let signed = self
                    .order_receipt(&order, &remote, review["listingId"].as_str())
                    .and_then(|receipt| self.sign_review(&order, receipt, review["rating"].as_u64().unwrap_or(5) as u8, review["text"].as_str().unwrap_or("")));
                let Ok(signed) = signed else { continue };
                let Ok(bytes) = signed.encode() else { continue };
                let carries = self.carries();
                self.reviews().merge(vec![signed.clone()], vec![], carries);
                record = Some(hex(&bytes));
                let id = signed.hash().map(|hash| hex(&hash)).unwrap_or_default();
                let written_on = signed.terms.written_on;
                let _ = self.orders().update(&order.id, |order| {
                    if let Some(remote) = order.remote.as_mut() {
                        remote["review"]["published"] = json!(true);
                        remote["review"]["record"] = json!(record);
                        remote["review"]["id"] = json!(id);
                        remote["review"]["writtenOn"] = json!(written_on);
                        remote["review"]["sentTo"] = json!([]);
                    }
                });
                changed += 1;
            }
            let Some(record) = record else { continue };
            changed += self.send_review(&order.id, &remote, &record);
        }
        changed
    }

    /// POST the review to the seller's node and every followed node not yet reached.
    fn send_review(&self, id: &str, remote: &Value, record: &str) -> usize {
        let reached: Vec<String> = self
            .orders()
            .get(id)
            .and_then(|order| order.remote.as_ref())
            .and_then(|remote| remote["review"]["sentTo"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|url| url.as_str().map(str::to_string))
            .collect();
        let mut targets: Vec<String> = remote["url"].as_str().map(str::to_string).into_iter().collect();
        targets.extend(self.replicas().follows.iter().map(|follow| follow.url.clone()));
        targets.dedup();
        let body = json!({ "v": 1, "reviews": [record] }).to_string();
        let sent: Vec<String> = targets
            .into_iter()
            .filter(|url| !reached.contains(url))
            .filter(|url| {
                crate::escrow_mail::loopback_request("POST", &format!("{url}{PUBLIC_REVIEWS}"), TIMEOUT)
                    .and_then(|request| {
                        request.set("Content-Type", "application/json").send_string(&body).map_err(|err| err.to_string())
                    })
                    .is_ok_and(|response| (200..300).contains(&response.status()))
            })
            .collect();
        if sent.is_empty() {
            return 0;
        }
        let _ = self.orders().update(id, |order| {
            if let Some(remote) = order.remote.as_mut() {
                let mut all = reached.clone();
                all.extend(sent.iter().cloned());
                remote["review"]["sentTo"] = json!(all);
            }
        });
        sent.len()
    }

    /// Pull the review pool of the node at `url`. Returns how many records were new.
    pub(crate) fn pull_reviews(&self, url: &str) -> usize {
        let fetched = crate::escrow_mail::loopback_request("GET", &format!("{url}{PUBLIC_REVIEWS}"), TIMEOUT)
            .ok()
            .and_then(|request| request.call().ok())
            .filter(|response| (200..300).contains(&response.status()))
            .and_then(|response| {
                let mut body = Vec::new();
                response.into_reader().take(MAX_BODY).read_to_end(&mut body).ok()?;
                serde_json::from_slice::<Value>(&body).ok()
            });
        fetched.map_or(0, |value| self.take_reviews(&value))
    }

    /// `POST /reviews/{id}/reply`: the own store answers one of its reviews.
    pub(crate) fn reply_to_review(&self, id: &str, body: &Value) -> Result<Value, String> {
        let hash = crate::stores::unhex32(id).ok_or("That review is not in this node's reviews.")?;
        let review = self.reviews().find(&hash).cloned().ok_or("That review is not in this node's reviews.")?;
        if review.store_id() != self.own_store {
            return Err("Only the store a review is about can reply to it.".into());
        }
        let text = body["text"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() || text.len() > MAX_REVIEW_TEXT {
            return Err("Write a reply of up to 1,000 characters.".into());
        }
        let secret = self.own_signing_key().ok_or("Unlock your wallet so your node can sign the reply.")?;
        let terms = Reply { store_id: self.own_store, review: hash, text, written_on: crate::own_store::today() };
        let reply = SignedReply::sign(terms, &secret).map_err(|err| match err {
            openbazaar_protocol::Error::PrivateText { .. } => "Leave contact details, addresses and links out of a reply; it is public.".to_string(),
            other => other.to_string(),
        })?;
        let carries = self.carries();
        self.reviews().merge(vec![], vec![reply], carries);
        Ok(self.reviews().store_value(&self.catalog().listings, &self.own_store))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    use serde_json::json;

    use crate::harness::TempDir;
    use crate::{load_catalog, NetworkMode, Node};

    #[test]
    fn a_direct_review_post_does_not_follow_a_redirect() {
        let trap = TcpListener::bind("127.0.0.1:0").unwrap();
        let trap_port = trap.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        thread::spawn(move || {
            for mut stream in trap.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        let seller = TcpListener::bind("127.0.0.1:0").unwrap();
        let seller_port = seller.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut stream in seller.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{trap_port}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = stream.write_all(body.as_bytes());
            }
        });

        let dir = TempDir::new();
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        let remote = json!({ "url": format!("http://127.0.0.1:{seller_port}") });
        let sent = node.send_review("no-such-order", &remote, "abcd");
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(sent, 0);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the review was posted to the redirect target");
    }
}
