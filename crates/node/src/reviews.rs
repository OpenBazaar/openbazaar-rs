//! Verified-purchase reviews, kept by every node.
//!
//! A review is signed by the per-order review key its receipt names, and
//! the receipt by the store, so any node can check it without trusting the
//! node it came from. Each node keeps a pool (`reviews.obv`, sealed with the
//! vault key) of the reviews it has seen, serves the whole pool at
//! `GET /public/reviews` without saying which ones its own user wrote, and
//! merges the pools of the nodes it follows. A review leaves a pool only
//! when the same order key signs a newer one, so a seller that leaves a
//! review out of its own pool cannot take it out of anyone else's.
//!
//! The store may attach one signed [`Reply`] per review; a newer reply from
//! the store replaces its older one. A reply names the review's hash, so a
//! review replaced by its buyer drops the reply to the old text.
//!
//! One review per paid order: the review key is per order, and the pool
//! keeps one review per key.

use std::path::{Path, PathBuf};

use openbazaar_protocol::{public_text, Listing, SignedReply, SignedReview, StoreId};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::vault::Vault;
use crate::Error;

const FILE: &str = "reviews.obv";
/// Reviews kept for one store this node carries in its catalog.
const MAX_PER_STORE: usize = 2_000;
/// Reviews kept for stores this node does not carry, all together. Anyone
/// can mint receipts for a store key of their own, so these stay few.
const MAX_UNCARRIED: usize = 2_000;
/// Records taken from one `POST /public/reviews` or one pull.
pub(crate) const MAX_BATCH: usize = 500;

/// The first of the month a payment confirmed in. A receipt is public in
/// every review, and a day would narrow which payment on chain it was.
pub(crate) fn paid_month(day: u32) -> u32 {
    day / 100 * 100 + 1
}

pub(crate) struct ReviewPool {
    path: PathBuf,
    vault: Vault,
    reviews: Vec<SignedReview>,
    replies: Vec<SignedReply>,
}

fn decode_all<T>(value: &Value, key: &str, decode: impl Fn(&[u8]) -> Option<T>) -> Vec<T> {
    value[key]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_BATCH)
        .filter_map(|record| record.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| decode(&bytes)))
        .collect()
}

pub(crate) fn decode_reviews(value: &Value) -> Vec<SignedReview> {
    decode_all(value, "reviews", |bytes| SignedReview::decode(bytes).ok())
}

pub(crate) fn decode_replies(value: &Value) -> Vec<SignedReply> {
    decode_all(value, "replies", |bytes| SignedReply::decode(bytes).ok())
}

/// The review checks out and its text could be published.
fn sound(review: &SignedReview) -> bool {
    review.verify_purchase().is_ok() && public_text::check("review", &review.terms.text).is_ok()
}

impl ReviewPool {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut pool = Self { path, vault, reviews: Vec::new(), replies: Vec::new() };
        if let Some(bytes) = pool.vault.read_sealed(&pool.path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            pool.reviews = decode_all(&value, "reviews", |bytes| SignedReview::decode(bytes).ok()).into_iter().filter(sound).collect();
            pool.replies = decode_all(&value, "replies", |bytes| SignedReply::decode(bytes).ok());
        }
        Ok(pool)
    }

    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.records()).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())
    }

    /// Every review and reply, hex-encoded, as `GET /public/reviews` serves them.
    pub(crate) fn records(&self) -> Value {
        let encode = |bytes: Result<Vec<u8>, openbazaar_protocol::Error>| bytes.ok().map(|bytes| hex(&bytes));
        let reviews: Vec<String> = self.reviews.iter().filter_map(|r| encode(r.encode())).collect();
        let replies: Vec<String> = self.replies.iter().filter_map(|r| encode(r.encode())).collect();
        json!({ "v": 1, "reviews": reviews, "replies": replies })
    }

    /// Take in what checks out. `carried` says whether this node carries a
    /// store. Returns how many records were new or newer.
    pub(crate) fn merge(&mut self, reviews: Vec<SignedReview>, replies: Vec<SignedReply>, carried: impl Fn(&StoreId) -> bool) -> usize {
        let mut changed = 0;
        for review in reviews.into_iter().filter(sound) {
            changed += usize::from(self.take_review(review, &carried));
        }
        for reply in replies {
            changed += usize::from(self.take_reply(reply));
        }
        if changed > 0 {
            if let Err(err) = self.save() {
                eprintln!("ob-node: {FILE}: {err}");
            }
        }
        changed
    }

    fn take_review(&mut self, review: SignedReview, carried: &impl Fn(&StoreId) -> bool) -> bool {
        let key = review.terms.receipt.terms.review_key;
        if let Some(old) = self.reviews.iter_mut().find(|old| old.terms.receipt.terms.review_key == key) {
            if review.terms.written_on <= old.terms.written_on || review.store_id() != old.store_id() {
                return false;
            }
            *old = review;
            return true;
        }
        let store = review.store_id();
        let full = if carried(&store) {
            self.reviews.iter().filter(|r| r.store_id() == store).count() >= MAX_PER_STORE
        } else {
            self.reviews.iter().filter(|r| !carried(&r.store_id())).count() >= MAX_UNCARRIED
        };
        if full {
            return false;
        }
        self.reviews.push(review);
        true
    }

    fn take_reply(&mut self, reply: SignedReply) -> bool {
        if !self.reviews.iter().any(|review| reply.answers(review)) || public_text::check("reply", &reply.terms.text).is_err() {
            return false;
        }
        match self.replies.iter_mut().find(|old| old.terms.review == reply.terms.review) {
            Some(old) if reply.terms.written_on > old.terms.written_on || (reply.terms.written_on == old.terms.written_on && *old != reply) => {
                *old = reply;
                true
            }
            Some(_) => false,
            None => {
                self.replies.push(reply);
                true
            }
        }
    }

    pub(crate) fn find(&self, id: &[u8; 32]) -> Option<&SignedReview> {
        self.reviews.iter().find(|review| review.hash().ok().as_ref() == Some(id))
    }

    fn reply_to(&self, review: &SignedReview) -> Option<&SignedReply> {
        self.replies.iter().find(|reply| reply.answers(review))
    }

    /// Leave out every review of `store`, as a seller's node may do to its own pool.
    #[cfg(test)]
    pub(crate) fn forget_store(&mut self, store: &StoreId) {
        self.reviews.retain(|review| review.store_id() != *store);
    }

    /// `GET /stores/{key}/reviews`: the store's reviews, newest first, with
    /// its replies, and the stars they add up to.
    pub(crate) fn store_value(&self, listings: &[Listing], store: &StoreId) -> Value {
        let mut reviews: Vec<&SignedReview> = self.reviews.iter().filter(|review| review.store_id() == *store).collect();
        reviews.sort_by_key(|review| std::cmp::Reverse(review.terms.written_on));
        let mut stars = [0u64; 5];
        for review in &reviews {
            stars[usize::from(review.terms.rating - 1)] += 1;
        }
        let count = reviews.len() as u64;
        let total: u64 = stars.iter().enumerate().map(|(i, n)| (i as u64 + 1) * n).sum();
        let average = if count == 0 { Value::Null } else { json!((total * 10 / count) as f64 / 10.0) };
        let items: Vec<Value> = reviews.into_iter().map(|review| self.review_value(listings, review)).collect();
        json!({ "storeId": store.to_hex(), "count": count, "average": average, "stars": stars, "reviews": items })
    }

    fn review_value(&self, listings: &[Listing], review: &SignedReview) -> Value {
        let listing_id = review.listing_id();
        let title = listings.iter().find(|listing| listing.id() == Some(listing_id)).map(|listing| listing.title.clone());
        let reply = self.reply_to(review).map(|reply| json!({ "text": reply.terms.text, "writtenOn": reply.terms.written_on }));
        json!({
            "id": review.hash().map(|hash| hex(&hash)).unwrap_or_default(),
            "listingId": listing_id.to_hex(),
            "listingTitle": title,
            "rating": review.terms.rating,
            "text": review.terms.text,
            "writtenOn": review.terms.written_on,
            "paidMonth": review.terms.receipt.terms.paid_on / 100,
            "reply": reply,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbazaar_protocol::{Receipt, Reply, Review, SignedReceipt};

    fn secret(label: u8) -> bitcoin::secp256k1::SecretKey {
        bitcoin::secp256k1::SecretKey::from_slice(&[label; 32]).unwrap()
    }

    fn xonly(secret: &bitcoin::secp256k1::SecretKey) -> [u8; 32] {
        secret.x_only_public_key(&bitcoin::secp256k1::Secp256k1::new()).0.serialize()
    }

    fn review(store: u8, order: u8, rating: u8, text: &str, on: u32) -> SignedReview {
        let store_secret = secret(store);
        let receipt = Receipt { store_id: StoreId::from_bytes(xonly(&store_secret)), listing_id: openbazaar_protocol::ListingId([7; 16]), review_key: xonly(&secret(order)), paid_on: 20261101 };
        let receipt = SignedReceipt::sign(receipt, &store_secret).unwrap();
        SignedReview::sign(Review { receipt, rating, text: text.into(), written_on: on }, &secret(order)).unwrap()
    }

    fn pool() -> (tempfile::TempDir, ReviewPool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = ReviewPool::open(dir.path(), Vault::open(dir.path(), &crate::VaultMode::File).unwrap()).unwrap();
        (dir, pool)
    }

    #[test]
    fn one_review_per_order_key_and_only_a_newer_one_from_that_key_replaces_it() {
        let (_dir, mut pool) = pool();
        let all = |_: &StoreId| true;
        assert_eq!(pool.merge(vec![review(1, 10, 5, "Lovely.", 20261110)], vec![], all), 1);
        assert_eq!(pool.merge(vec![review(1, 10, 5, "Lovely.", 20261110)], vec![], all), 0, "a copy changes nothing");
        assert_eq!(pool.merge(vec![review(1, 10, 1, "Older text.", 20261109)], vec![], all), 0, "an older review does not win");
        assert_eq!(pool.merge(vec![review(1, 10, 4, "Chipped after a week.", 20261120)], vec![], all), 1);
        let store = review(1, 10, 4, "", 20261120).store_id();
        let value = pool.store_value(&[], &store);
        assert_eq!((value["count"].as_u64(), value["reviews"][0]["rating"].as_u64()), (Some(1), Some(4)));
    }

    #[test]
    fn altered_text_and_a_reply_from_another_key_are_refused() {
        let (_dir, mut pool) = pool();
        let mut altered = review(1, 10, 1, "Never arrived.", 20261110);
        altered.terms.text = "Great seller!".into();
        assert_eq!(pool.merge(vec![altered], vec![], |_| true), 0);

        let kept = review(1, 10, 1, "Never arrived.", 20261110);
        pool.merge(vec![kept.clone()], vec![], |_| true);
        let reply = |by: u8, text: &str| {
            let terms = Reply { store_id: kept.store_id(), review: kept.hash().unwrap(), text: text.into(), written_on: 20261111 };
            SignedReply::sign(terms, &secret(by))
        };
        assert!(reply(2, "It did.").is_err(), "only the store's key signs its reply");
        assert_eq!(pool.merge(vec![], vec![reply(1, "Tracking shows it delivered.").unwrap()], |_| true), 1);
        let value = pool.store_value(&[], &kept.store_id());
        assert_eq!(value["reviews"][0]["text"], "Never arrived.");
        assert_eq!(value["reviews"][0]["reply"]["text"], "Tracking shows it delivered.");
    }

    #[test]
    fn a_receipt_names_the_month_it_was_paid_in() {
        assert_eq!(paid_month(20261117), 20261101);
        assert_eq!(paid_month(20261201), 20261201);
    }
}
