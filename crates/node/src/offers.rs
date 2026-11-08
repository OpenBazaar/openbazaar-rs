//! Bids waiting on the seller, and the signed answer to each one.
//!
//! A buyer sends one bid with `POST /offers`. The node seals it as a message
//! (`POST /mail` on the seller), signed by a fresh conversation key, carrying
//! the encoded [`Bid`] and the same reply route an order uses. The seller's
//! paired client lists them at `GET /offers` and answers one at
//! `POST /offers/{id}/answer`: a decline, or an [`Offer`]
//! signed by the store key and bound to the conversation key that sent the
//! bid. The answer goes back in the sealed reply. Accepting it is ordering
//! with that record, which the order check already does. The seller's floor
//! stays on this node; nothing here publishes it.

use std::path::{Path, PathBuf};

use bitcoin::hashes::{sha256, Hash, HashEngine};
use openbazaar_protocol::{Bid, Listing, Offer, SignedOffer, VariantId};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};
use tormail::{seal_message, ReplyPublic};

use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::Sent;
use crate::order_mail::{mail_key, MailError, STORE_MAIL};
use crate::own_listings::{capitalize, StoreError};
use crate::sales::{refused, retry};
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "offers.obv";
const MAX_PENDING: usize = 200;
const MAX_ANSWERED: usize = 1_000;
const MAX_REASON: usize = 1_000;
const NOT_TAKING: &str = "This listing is not taking offers.";
const NO_LISTING: &str = "That listing is not in this store.";
const BAD_VARIANT: &str = "That option is not on this listing.";
const BAD_CURRENCY: &str = "That bid is in a different currency.";

#[derive(Clone)]
struct BidRow {
    id: [u8; 16],
    bid_bytes: Vec<u8>,
    buyer: [u8; 32],
    reply_url: String,
    reply_key: [u8; 32],
    drop: [u8; 16],
}

pub(crate) struct OfferBook {
    path: PathBuf,
    vault: Vault,
    bids: Vec<BidRow>,
    /// Fingerprints of bids already answered, so a redelivery is not a new bid.
    answered: Vec<String>,
}

impl OfferBook {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut book = Self { path, vault, bids: Vec::new(), answered: Vec::new() };
        let Some(bytes) = book.vault.read_sealed(&book.path)? else {
            return Ok(book);
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
        for row in value["bids"].as_array().into_iter().flatten() {
            match row_of(row) {
                Some(row) => book.bids.push(row),
                None => eprintln!("ob-node: {FILE}: dropped a bid that could not be read"),
            }
        }
        for id in value["answered"].as_array().into_iter().flatten() {
            if let Some(id) = id.as_str() {
                book.answered.push(id.to_string());
            }
        }
        Ok(book)
    }

    fn save(&self) -> Result<(), StoreError> {
        let bids: Vec<Value> = self
            .bids
            .iter()
            .map(|row| {
                json!({
                    "id": hex(&row.id),
                    "bid": hex(&row.bid_bytes),
                    "buyer": hex(&row.buyer),
                    "replyUrl": row.reply_url,
                    "replyKey": hex(&row.reply_key),
                    "drop": hex(&row.drop),
                })
            })
            .collect();
        let bytes = serde_json::to_vec(&json!({ "v": 1, "bids": bids, "answered": self.answered })).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| StoreError::Failed(err.to_string()))
    }
}

fn row_of(value: &Value) -> Option<BidRow> {
    let id = crate::escrow_mail::parse_id(value["id"].as_str()?)?;
    let bid_bytes = unhex(value["bid"].as_str()?).ok()?;
    Bid::decode(&bid_bytes).ok()?;
    Some(BidRow {
        id,
        bid_bytes,
        buyer: crate::stores::unhex32(value["buyer"].as_str()?)?,
        reply_url: value["replyUrl"].as_str()?.to_string(),
        reply_key: crate::stores::unhex32(value["replyKey"].as_str()?)?,
        drop: crate::escrow_mail::parse_id(value["drop"].as_str()?)?,
    })
}

fn fingerprint(buyer: &[u8; 32], bid: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(b"ob-node/offer-bid/v1");
    engine.input(buyer);
    engine.input(bid);
    sha256::Hash::from_engine(engine).to_byte_array()
}

fn variant_ok(listing: &Listing, id: VariantId) -> bool {
    match listing.commerce.as_ref() {
        Some(commerce) if !commerce.variants.variants.is_empty() => commerce.variants.get(id).is_some(),
        _ => id == VariantId::NONE,
    }
}

fn currency_of(listing: &Listing) -> Option<&str> {
    listing.details.as_ref()?.price.as_ref().map(|price| price.currency.as_str())
}

fn reply_route(body: &Value) -> Result<(String, [u8; 32], [u8; 16]), MailError> {
    let url = crate::peer_http::sale_base(body["replyUrl"].as_str().unwrap_or("")).map_err(|_| refused("that bid has no reply route"))?;
    let key = crate::stores::unhex32(body["replyKey"].as_str().unwrap_or("")).ok_or_else(|| refused("that bid has no reply route"))?;
    let drop = crate::escrow_mail::parse_id(body["drop"].as_str().unwrap_or("")).ok_or_else(|| refused("that bid has no reply route"))?;
    ReplyPublic::from_bytes(key).map_err(|_| refused("that bid has no reply route"))?;
    Ok((url, key, drop))
}

fn public_bid(row: &BidRow) -> Option<Value> {
    let bid = Bid::decode(&row.bid_bytes).ok()?;
    Some(json!({
        "id": hex(&row.id),
        "listingId": bid.listing_id.to_hex(),
        "variantId": bid.variant_id.to_hex(),
        "quantity": bid.quantity,
        "amount": bid.amount,
        "currency": bid.currency,
    }))
}

impl Node {
    pub(crate) fn offers(&self) -> std::sync::MutexGuard<'_, OfferBook> {
        self.offers.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// `GET /offers`: bids waiting for an answer. No conversation key and no reply route.
    pub(crate) fn own_offers_value(&self) -> Value {
        let offers: Vec<Value> = self.offers().bids.iter().filter_map(public_bid).collect();
        json!({ "offers": offers })
    }

    /// `POST /offers`: seal one bid to the listing's store and send it.
    ///
    /// The conversation key is derived for this bid alone, the same way an
    /// order letter is signed, so the bid does not link to the buyer's other
    /// orders. The answer names that id only to the paired buyer.
    pub(crate) fn submit_bid(&self, body: &Value) -> Result<Value, StoreError> {
        let amount = match body.get("amount") {
            Some(Value::Number(n)) => n.as_u64().filter(|n| *n > 0),
            _ => None,
        }
        .ok_or_else(|| StoreError::Invalid("Enter an amount.".into()))?;
        let quantity = body["quantity"].as_u64().filter(|n| (1..=999).contains(n)).ok_or_else(|| StoreError::Invalid("An offer is for 1 to 999.".into()))? as u16;
        let currency = body["currency"].as_str().unwrap_or("").trim();
        if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(StoreError::Invalid(BAD_CURRENCY.into()));
        }
        let hash = body["listingHash"].as_str().unwrap_or("").trim();
        let (listing, vendor) = {
            let catalog = self.catalog();
            let listing = catalog
                .listings
                .iter()
                .find(|listing| listing.content_hash().ok().is_some_and(|sum| sum.to_hex() == hash))
                .cloned()
                .ok_or_else(|| StoreError::Invalid("That listing is not in this shop.".into()))?;
            if listing.store_id == self.own_store {
                return Err(StoreError::Invalid("That is your own store.".into()));
            }
            let store = catalog.store_of(&listing).cloned().ok_or_else(|| StoreError::Invalid("That seller has no store record.".into()))?;
            let vendor = crate::stores::mailbox_key(&store).map_err(StoreError::Failed)?;
            (listing, vendor)
        };
        let listing_id = listing.id().ok_or_else(|| StoreError::Invalid("That listing cannot take an offer.".into()))?;
        let variant = match body.get("variant").and_then(Value::as_str).filter(|text| !text.is_empty()) {
            None => VariantId::NONE,
            Some(text) => {
                let bytes = unhex(text).ok().and_then(|bytes| <[u8; 8]>::try_from(bytes).ok()).ok_or_else(|| StoreError::Invalid(BAD_VARIANT.into()))?;
                VariantId(bytes)
            }
        };
        if !variant_ok(&listing, variant) {
            return Err(StoreError::Invalid(BAD_VARIANT.into()));
        }
        if currency_of(&listing) != Some(currency) {
            return Err(StoreError::Invalid(BAD_CURRENCY.into()));
        }
        if !listing.commerce.as_ref().is_some_and(|commerce| commerce.pricing.accepts_offers) {
            return Err(StoreError::Invalid(NOT_TAKING.into()));
        }
        let url = self.replicas().url_of(&listing.store_id).map(str::to_string).ok_or_else(|| StoreError::Invalid("Follow that store before making an offer.".into()))?;
        let id = crate::orders::random_id();
        let store_key = *listing.store_id.as_bytes();
        let (reply, drop) = self.order_reply_secret(&id, &store_key).map_err(|err| self.letter_err(err))?;
        let from = self.order_letter_key(&id, &store_key).map_err(|err| self.letter_err(err))?;
        let per_order = if self.remote.services.sale_onion().is_some() {
            let seed = self.order_reply_onion_seed(&id, &store_key).map_err(|err| self.letter_err(err))?;
            Some(self.remote.publish_reply_onion(&self.data_dir, &seed).map_err(StoreError::Failed)?)
        } else {
            None
        };
        let reply_url = crate::letter_auth::choose_reply(per_order.as_deref(), self.sale_reply_url().as_deref()).ok_or_else(|| StoreError::Invalid("This node has no address for the seller's reply.".into()))?;
        let bid = Bid { listing_id, variant_id: variant, quantity, amount, currency: currency.to_string() };
        let encoded = bid.encode().map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let message = json!({
            "v": 1,
            "kind": "bid",
            "bid": hex(&encoded),
            "replyUrl": reply_url,
            "replyKey": hex(reply.public_key().as_bytes()),
            "drop": hex(&drop.0),
        });
        let bytes = serde_json::to_vec(&message).map_err(|err| StoreError::Failed(err.to_string()))?;
        let letter = seal_message(&bytes, &vendor, &from).map_err(|err| StoreError::Failed(err.to_string()))?;
        let target = format!("{url}{STORE_MAIL}");
        let conversation = [id.as_bytes(), store_key.as_slice()].concat();
        let posted = letter.to_bytes();
        match self.order_outbox.send(mail_key(&target, &conversation, &bytes), &target, &posted) {
            Ok(Sent::Delivered | Sent::Queued(_)) => Ok(json!({ "id": id })),
            Err(err) => Err(StoreError::Invalid(format!("The seller did not take that offer. {err}"))),
        }
    }

    fn letter_err(&self, err: String) -> StoreError {
        match self.lock.state() {
            crate::wallet_lock::State::Unlocked => StoreError::Failed(err),
            state => StoreError::Locked(state.as_str()),
        }
    }

    #[cfg(test)]
    pub(crate) fn bid_sender_for(&self, listing_id: &str) -> Option<[u8; 32]> {
        let id = crate::own_listings::parse_id(listing_id)?;
        self.offers().bids.iter().find_map(|row| {
            let bid = Bid::decode(&row.bid_bytes).ok()?;
            (bid.listing_id == id).then_some(row.buyer)
        })
    }

    /// A sealed bid. The same bid from the same conversation key is one row.
    pub(crate) fn receive_bid(&self, buyer: &[u8; 32], body: &Value) -> Result<(), MailError> {
        let raw = unhex(body["bid"].as_str().unwrap_or("")).map_err(|_| refused("that bid could not be read"))?;
        let bid = Bid::decode(&raw).map_err(|_| refused("that bid could not be read"))?;
        let bid_bytes = bid.encode().map_err(|_| refused("that bid could not be read"))?;
        let (url, key, drop) = reply_route(body)?;
        let problem = if self.on_vacation() {
            Some(crate::vacation::AWAY_MESSAGE)
        } else {
            let own = self.own_listings();
            match own.by_id(&bid.listing_id) {
                None => Some(NO_LISTING),
                Some(listing) if !variant_ok(listing, bid.variant_id) => Some(BAD_VARIANT),
                Some(listing) if currency_of(listing) != Some(bid.currency.as_str()) => Some(BAD_CURRENCY),
                Some(listing) if !listing.commerce.as_ref().is_some_and(|c| c.pricing.accepts_offers) => Some(NOT_TAKING),
                Some(_) => None,
            }
        };
        if let Some(reason) = problem {
            return self.queue_buyer_reply(&url, key, drop, &json!({ "v": 1, "kind": "declined", "reason": reason })).map_err(retry);
        }
        let seen = hex(&fingerprint(buyer, &bid_bytes));
        let mut book = self.offers();
        if book.answered.iter().any(|id| id == &seen) || book.bids.iter().any(|row| hex(&fingerprint(&row.buyer, &row.bid_bytes)) == seen) {
            return Ok(());
        }
        if book.bids.len() >= MAX_PENDING {
            return Err(refused("that store cannot take another bid right now"));
        }
        let mut id = [0u8; 16];
        loop {
            StdRng::from_os_rng().fill_bytes(&mut id);
            if id != [0; 16] && book.bids.iter().all(|row| row.id != id) {
                break;
            }
        }
        book.bids.push(BidRow { id, bid_bytes, buyer: *buyer, reply_url: url, reply_key: key, drop });
        if let Err(err) = book.save() {
            book.bids.pop();
            return Err(retry(store_detail(err)));
        }
        Ok(())
    }

    /// `POST /offers/{id}/answer`: decline, or sign an offer for the conversation key that bid.
    pub(crate) fn answer_offer(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let id = crate::escrow_mail::parse_id(id).ok_or(StoreError::NotFound)?;
        let row = self.offers().bids.iter().find(|row| row.id == id).cloned().ok_or(StoreError::NotFound)?;
        let declining = matches!(body.get("decline"), Some(value) if !value.is_null());
        let pricing = matches!(body.get("amount"), Some(value) if !value.is_null()) || matches!(body.get("expiresAt"), Some(value) if !value.is_null());
        if declining && pricing {
            return Err(StoreError::Invalid("Decline the bid or answer it, not both.".into()));
        }
        if declining {
            let reason = decline_reason(body)?;
            self.queue_buyer_reply(&row.reply_url, row.reply_key, row.drop, &json!({ "v": 1, "kind": "declined", "reason": reason }))
                .map_err(StoreError::Failed)?;
            self.finish_bid(&row)?;
            return Ok(json!({ "id": hex(&row.id), "declined": true }));
        }
        let bid = Bid::decode(&row.bid_bytes).map_err(|err| StoreError::Failed(err.to_string()))?;
        let listing = self
            .own_listings()
            .by_id(&bid.listing_id)
            .cloned()
            .ok_or_else(|| StoreError::Invalid(NO_LISTING.into()))?;
        if !variant_ok(&listing, bid.variant_id) {
            return Err(StoreError::Invalid(BAD_VARIANT.into()));
        }
        if currency_of(&listing) != Some(bid.currency.as_str()) {
            return Err(StoreError::Invalid(BAD_CURRENCY.into()));
        }
        let amount = match body.get("amount") {
            None | Some(Value::Null) => bid.amount,
            Some(value) => value.as_u64().filter(|n| *n > 0).ok_or_else(|| StoreError::Invalid("The amount is not a valid number.".into()))?,
        };
        let expires_at = body["expiresAt"].as_u64().ok_or_else(|| StoreError::Invalid("An offer needs an expiry.".into()))?;
        if expires_at <= crate::escrow_outbox::now_secs() {
            return Err(StoreError::Invalid("The offer's expiry is already past.".into()));
        }
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let counter = amount != bid.amount;
        let signed = SignedOffer::sign(
            Offer {
                store_id: self.own_store,
                offer_id: row.id,
                listing_id: bid.listing_id,
                variant_id: bid.variant_id,
                quantity: bid.quantity,
                amount,
                currency: bid.currency.clone(),
                buyer_key: row.buyer,
                expires_at,
                counter,
            },
            &secret,
        )
        .map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let bytes = signed.encode().map_err(|err| StoreError::Failed(err.to_string()))?;
        self.queue_buyer_reply(&row.reply_url, row.reply_key, row.drop, &json!({ "v": 1, "kind": "offer", "offer": hex(&bytes) }))
            .map_err(StoreError::Failed)?;
        self.finish_bid(&row)?;
        Ok(json!({
            "id": hex(&row.id),
            "offer": hex(&bytes),
            "amount": amount,
            "currency": bid.currency,
            "expiresAt": expires_at,
            "counter": counter,
        }))
    }

    fn finish_bid(&self, row: &BidRow) -> Result<(), StoreError> {
        let mut book = self.offers();
        let before = (book.bids.clone(), book.answered.clone());
        book.bids.retain(|bid| bid.id != row.id);
        book.answered.push(hex(&fingerprint(&row.buyer, &row.bid_bytes)));
        if book.answered.len() > MAX_ANSWERED {
            let extra = book.answered.len() - MAX_ANSWERED;
            book.answered.drain(0..extra);
        }
        if let Err(err) = book.save() {
            book.bids = before.0;
            book.answered = before.1;
            return Err(err);
        }
        Ok(())
    }
}

fn store_detail(err: StoreError) -> String {
    match err {
        StoreError::Invalid(message) | StoreError::Failed(message) => message,
        StoreError::NotFound => "not found".into(),
        StoreError::Locked(state) => state.into(),
    }
}

fn decline_reason(body: &Value) -> Result<String, StoreError> {
    let text = body["decline"].as_str().unwrap_or("").trim();
    if text.is_empty() {
        return Err(StoreError::Invalid("Say why the bid is declined.".into()));
    }
    if text.len() > MAX_REASON || text.contains('\0') {
        return Err(StoreError::Invalid("The decline is too long.".into()));
    }
    Ok(text.to_string())
}
