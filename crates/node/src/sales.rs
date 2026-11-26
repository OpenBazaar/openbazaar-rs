//! The seller's side of an order from another node.
//!
//! An order letter arrives at `POST /mail` ([`crate::order_mail`]). The node
//! opens it with the store mailbox key and prices it against the own store's
//! listings: a fiat price at the node's rate now, else the signed sats price.
//! It takes a fresh address from the node wallet, keeps the sale in
//! `sales.obv` (sealed: it holds the buyer's ship-to), and replies with the
//! terms. All of that is receive tier, so a locked node still takes orders.
//!
//! Sale: Awaiting payment -> Payment seen -> Paid (the watcher, which tells
//! the buyer) -> Shipped (the seller fulfills, with tracking or the digital
//! goods) -> Completed (the buyer confirms receipt).

use openbazaar_protocol::{Listing, ListingKind, Returns};
use serde_json::{json, Value};
use tormail::{open_message, open_order, reply_frame, seal_reply, DropId, Opened, Order, ReplyPublic, SealedLetter, StoreSecret};

use crate::escrow_mail::hex;
use crate::order_mail::{body_bytes, mail_key, MailError, REPLY_MAIL};
use crate::own_listings::StoreError;
use crate::vault::Vault;
use crate::watch::{direct_status, AWAITING_PAYMENT, COMPLETED, PAID, PAYMENT_SEEN, SHIPPED};
use crate::{Error, Node};

const FILE: &str = "sales.obv";
const MAX_LINE_QUANTITY: u32 = 99;

pub(crate) struct Sales {
    path: std::path::PathBuf,
    vault: Vault,
    items: Vec<Value>,
}

impl Sales {
    pub(crate) fn open(dir: &std::path::Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let items = match vault.read_sealed(&path)? {
            Some(bytes) => serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| value["sales"].as_array().cloned())
                .ok_or_else(|| Error::Detail(format!("{FILE} could not be read")))?,
            None => Vec::new(),
        };
        Ok(Self { path, vault, items })
    }

    fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&json!({ "v": 1, "sales": self.items })).map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Value> {
        self.items.iter().find(|sale| sale["id"] == id)
    }

    fn insert(&mut self, sale: Value) -> Result<(), String> {
        self.items.push(sale);
        if let Err(err) = self.save() {
            self.items.pop();
            return Err(err);
        }
        Ok(())
    }

    /// Change one sale and write the file; the change is undone if the write fails.
    pub(crate) fn update(&mut self, id: &str, change: impl FnOnce(&mut Value)) -> Result<Option<Value>, String> {
        let Some(index) = self.items.iter().position(|sale| sale["id"] == id) else {
            return Ok(None);
        };
        let before = self.items[index].clone();
        change(&mut self.items[index]);
        if self.items[index] == before {
            return Ok(Some(before));
        }
        if let Err(err) = self.save() {
            self.items[index] = before;
            return Err(err);
        }
        Ok(Some(self.items[index].clone()))
    }

    pub(crate) fn list(&self) -> impl Iterator<Item = &Value> {
        self.items.iter()
    }
}

/// What the seller UI sees of a sale: everything but the reply route.
fn public_sale(sale: &Value) -> Value {
    let mut out = sale.clone();
    if let Some(map) = out.as_object_mut() {
        for key in ["replyUrl", "replyKey", "buyerSender"] {
            map.remove(key);
        }
    }
    out
}

pub(crate) fn refused(message: impl Into<String>) -> MailError {
    MailError::Refused(message.into())
}

pub(crate) fn unauthorized() -> MailError {
    MailError::Unauthorized("That order status request was not authorized.".into())
}

pub(crate) fn retry(message: impl std::fmt::Display) -> MailError {
    MailError::Retry(message.to_string())
}

/// The lines of an order: the first listing, then every option named by a listing hash.
fn order_lines(order: &Order) -> Result<Vec<(String, u32)>, String> {
    let mut lines = vec![(hex(order.listing_hash.as_bytes()), order.quantity)];
    for (name, value) in &order.options {
        if name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            lines.push((name.clone(), value.parse().map_err(|_| "A quantity in this order is not a number.".to_string())?));
        }
    }
    if lines.iter().any(|(_, quantity)| *quantity == 0 || *quantity > MAX_LINE_QUANTITY) {
        return Err(format!("Order between 1 and {MAX_LINE_QUANTITY} of each listing."));
    }
    Ok(lines)
}

impl Node {
    pub(crate) fn sales(&self) -> std::sync::MutexGuard<'_, Sales> {
        self.sales.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn own_sales_value(&self) -> Value {
        let sales = self.sales();
        let mut items: Vec<Value> = sales.list().map(public_sale).collect();
        items.sort_by(|a, b| b["created"].as_u64().cmp(&a["created"].as_u64()));
        json!({ "sales": items })
    }

    pub(crate) fn sale_value(&self, id: &str) -> Option<Value> {
        self.sales().get(id).map(public_sale)
    }

    /// `POST /mail`: an order letter or a buyer's follow-up, sealed to the own store.
    /// Returns an immediate sealed reply for pull-mode orders (where the buyer
    /// runs no 24/7 reply mailbox), or `None` for push-mode orders and messages.
    pub(crate) fn receive_store_mail(&self, bytes: &[u8]) -> Result<Option<tormail::SealedReply>, MailError> {
        let letter = SealedLetter::from_bytes(bytes).map_err(|_| refused("that is not a sealed letter"))?;
        let secret = {
            let wallet = self.buyer_wallet();
            let key = wallet.watch.store_mailbox_key().map_err(retry)?;
            StoreSecret::from_bytes(*key).map_err(retry)?
        };
        if let Ok(opened) = open_order(&letter, &secret) {
            return self.receive_order(opened);
        }
        let opened = open_message(&letter, &secret).map_err(|_| refused("that letter is not for this store"))?;
        self.receive_buyer_message(opened)?;
        Ok(None)
    }

    fn receive_order(&self, opened: Opened<Order>) -> Result<Option<tormail::SealedReply>, MailError> {
        let order = opened.body;
        let id = hex(&order.reply.drop.0);
        let sender = hex(&opened.sender.0);
        if let Some(sale) = self.sales().get(&id).cloned() {
            return if sale["buyerSender"] == sender.as_str() {
                if sale["replyUrl"].as_str().unwrap_or("").is_empty() {
                    let route = Route::of(&sale)?;
                    let terms = self.terms_reply_for(&sale);
                    return self.seal_sale_reply(route.key, route.drop, &terms).map(Some).map_err(retry);
                }
                Ok(None)
            } else {
                Err(refused("that order id is taken"))
            };
        }
        let raw_reply_url = order.options.get("replyUrl").map(|url| url.trim()).unwrap_or("");
        let pull_mode = raw_reply_url.is_empty();
        let reply_url = if pull_mode {
            String::new()
        } else {
            crate::peer_http::sale_base(raw_reply_url).map_err(refused)?
        };
        let route = Route { url: reply_url, key: *order.reply.key.as_bytes(), drop: order.reply.drop.0 };
        if self.on_vacation() {
            let declined = json!({ "v": 1, "kind": "declined", "reason": crate::vacation::AWAY_MESSAGE });
            if pull_mode {
                return self.seal_sale_reply(route.key, route.drop, &declined).map(Some).map_err(retry);
            }
            self.queue_reply(&route, &declined).map_err(retry)?;
            return Ok(None);
        }
        let rail = order.options.get("rail").map(String::as_str);
        let priced = match (match rail {
            Some("zec") => self.zec_sale_refusal(),
            Some("xmr") => self.xmr_sale_refusal(),
            Some("ln") => self.ln_sale_refusal(),
            _ => None,
        }, order.terms.lines.is_empty()) {
            (Some(reason), _) => Err(reason.to_string()),
            (None, true) => self.price_order(&order),
            (None, false) => self.price_terms_order(&id, &order, &opened.sender.0)?,
        };
        match priced {
            Ok(mut priced) => {
                let checked = priced.checked.take();
                let result = self.open_sale(id.clone(), sender, &order, route, priced, rail);
                match (&result, checked) {
                    (Ok(_), Some(checked)) => self.record_terms(&id, &checked),
                    (Err(_), Some(_)) => self.release_units(&id),
                    _ => {}
                }
                result
            }
            Err(reason) => {
                let declined = json!({ "v": 1, "kind": "declined", "reason": reason });
                if pull_mode {
                    return self.seal_sale_reply(route.key, route.drop, &declined).map(Some).map_err(retry);
                }
                self.queue_reply(&route, &declined).map_err(retry)?;
                Ok(None)
            }
        }
    }

    /// The order's lines priced now, or why the store declines it.
    fn price_order(&self, order: &Order) -> Result<Priced, String> {
        let lines = order_lines(order)?;
        if order.options.get("settlement").map(String::as_str) != Some("direct") {
            return Err("This store takes direct payment only.".into());
        }
        let own = self.own_listings();
        let mut priced = Vec::new();
        let mut values = Vec::new();
        for (hash, quantity) in &lines {
            let listing = own.by_hash(hash).ok_or("A listing in this order is no longer for sale.")?;
            if listing.stock < *quantity {
                return Err(format!("Only {} of {} are left.", listing.stock, listing.title));
            }
            let now = self.priced_now(listing);
            values.push(json!({
                "listingHash": hash,
                "listingId": listing.id().map(|id| id.to_hex()),
                "title": listing.title,
                "kind": listing.kind.as_str(),
                "quantity": quantity,
                "priceSats": now.price_sats,
            }));
            priced.push((now, *quantity));
        }
        drop(own);
        let refs: Vec<(&Listing, u32)> = priced.iter().map(|(listing, quantity)| (listing, *quantity)).collect();
        let (items, shipping) = crate::orders::package_shipping(&refs).map_err(|err| err.to_string())?;
        let digital = priced.iter().any(|(listing, _)| listing.kind == ListingKind::Digital);
        let physical = priced.iter().any(|(listing, _)| listing.kind == ListingKind::Physical);
        let policies: Vec<Returns> = priced
            .iter()
            .map(|(listing, _)| listing.commerce.as_ref().map(|commerce| commerce.returns).unwrap_or_default())
            .collect();
        let return_policy = self.return_policy_of(&policies);
        Ok(Priced { lines: values, items, shipping, digital, physical, quote: Value::Null, checked: None, return_policy })
    }

    /// Keep the sale and reply with its terms. A Zcash or Monero sale has a `payment` and no bitcoin `address`, so [`Self::watch_sales`] skips it.
    fn open_sale(&self, id: String, sender: String, order: &Order, route: Route, priced: Priced, rail: Option<&str>) -> Result<Option<tormail::SealedReply>, MailError> {
        let total = priced.items.checked_add(priced.shipping).ok_or_else(|| refused("the total overflows"))?;
        let payment = match rail {
            Some("zec") => Some(self.zec_sale_payment(total).map_err(retry)?),
            Some("xmr") => Some(self.xmr_sale_payment(total).map_err(retry)?),
            Some("ln") => Some(self.ln_sale_payment(total, &id).map_err(retry)?),
            _ => None,
        };
        let address = match payment {
            Some(_) => None,
            None => Some(self.buyer_wallet().fresh_address().map_err(retry)?.to_string()),
        };
        let (ship_to, note) = match (priced.physical, priced.digital) {
            (true, true) => (order.ship_to.clone(), order.options.get("delivery").cloned().unwrap_or_default()),
            (true, false) => (order.ship_to.clone(), String::new()),
            _ => (String::new(), order.ship_to.clone()),
        };
        let mut sale = json!({
            "id": id,
            "created": crate::escrow_outbox::now_secs(),
            "status": AWAITING_PAYMENT,
            "lines": priced.lines,
            "itemsSats": priced.items,
            "shippingSats": priced.shipping,
            "totalSats": total,
            "address": address,
            "shipTo": ship_to,
            "deliveryNote": note,
            "receivedSats": 0,
            "confirmations": 0,
            "txids": [],
            "network": self.network.as_str(),
            "replyUrl": route.url,
            "replyKey": hex(&route.key),
            "buyerSender": sender,
            "quote": priced.quote,
        });
        let mut terms = json!({
            "v": 1,
            "kind": "terms",
            "address": address,
            "amountSats": total,
            "itemsSats": priced.items,
            "shippingSats": priced.shipping,
            "network": self.network.as_str(),
        });
        if !priced.quote.is_null() {
            terms["quote"] = priced.quote;
        }
        if let Some(policy) = &priced.return_policy {
            sale["returnPolicy"] = json!(policy);
            terms["returnPolicy"] = json!(policy);
        }
        if let Some(payment) = payment {
            match payment["rail"].as_str() {
                Some("zec") => crate::zec::add_terms(&mut terms, &payment),
                Some("xmr") => crate::xmr::add_terms(&mut terms, &payment),
                Some("ln") => crate::lightning::add_terms(&mut terms, &payment),
                _ => {}
            }
            sale["payment"] = payment;
        }
        self.sales().insert(sale).map_err(retry)?;
        if route.url.is_empty() {
            let sealed = self.seal_sale_reply(route.key, route.drop, &terms).map_err(retry)?;
            return Ok(Some(sealed));
        }
        self.queue_reply(&route, &terms).map_err(retry)?;
        Ok(None)
    }

    fn terms_reply_for(&self, sale: &Value) -> Value {
        let mut terms = json!({
            "v": 1,
            "kind": "terms",
            "address": sale["address"],
            "amountSats": sale["totalSats"],
            "itemsSats": sale["itemsSats"],
            "shippingSats": sale["shippingSats"],
            "network": sale["network"],
        });
        if !sale["quote"].is_null() {
            terms["quote"] = sale["quote"].clone();
        }
        if let Some(policy) = sale["returnPolicy"].as_str() {
            terms["returnPolicy"] = json!(policy);
        }
        if let Some(payment) = sale.get("payment").filter(|v| !v.is_null()) {
            match payment["rail"].as_str() {
                Some("zec") => crate::zec::add_terms(&mut terms, payment),
                Some("xmr") => crate::xmr::add_terms(&mut terms, payment),
                Some("ln") => crate::lightning::add_terms(&mut terms, payment),
                _ => {}
            }
        }
        terms
    }

    /// `POST /mail/pull`: authenticated pull of a sale's sealed status snapshot.
    ///
    /// The request is a [`SealedLetter`] sealed to this store's mailbox key and
    /// signed inside the ciphertext by the buyer's per-order key. Any unknown
    /// sale id or mismatched buyer key receives a uniform unauthorized error.
    ///
    /// Rate-limited to one pull per sale per 30 seconds to bound timing
    /// information the buyer can extract from pull frequency (#251).
    pub(crate) fn pull_sale_status(&self, bytes: &[u8]) -> Result<Value, MailError> {
        use std::collections::HashMap;
        use std::sync::Mutex;
        use std::time::Instant;

        /// Minimum interval between pulls for the same sale.
        const PULL_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

        static PULL_TIMES: std::sync::LazyLock<Mutex<HashMap<String, Instant>>> =
            std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

        let letter = SealedLetter::from_bytes(bytes).map_err(|_| refused("that is not a sealed letter"))?;
        let secret = {
            let wallet = self.buyer_wallet();
            let key = wallet.watch.store_mailbox_key().map_err(retry)?;
            StoreSecret::from_bytes(*key).map_err(retry)?
        };
        let opened = open_message(&letter, &secret).map_err(|_| unauthorized())?;
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| refused("that pull request could not be read"))?;
        if body["v"].as_u64() != Some(1) || body["kind"].as_str() != Some("pull") {
            return Err(refused("that is not an order status pull request"));
        }
        let sale_id = body["sale"].as_str().unwrap_or("");
        let nonce = body["nonce"].as_str().unwrap_or("");
        if crate::escrow_mail::parse_id(sale_id).is_none() || crate::escrow_mail::parse_id(nonce).is_none() {
            return Err(refused("that pull request is missing a valid sale id or nonce"));
        }
        let sender = hex(&opened.sender.0);
        let Some(sale) = self.sales().get(sale_id).cloned() else {
            return Err(unauthorized());
        };
        if sale["buyerSender"].as_str() != Some(sender.as_str()) {
            return Err(unauthorized());
        }
        // Per-sale pull rate limit: at most one successful pull per 10 seconds.
        // Checked after authorization so unauthorized requests do not burn the
        // legitimate buyer's quota.
        {
            let mut times = PULL_TIMES.lock().unwrap_or_else(|err| err.into_inner());
            let now = Instant::now();
            if let Some(last) = times.get(sale_id) {
                if now.saturating_duration_since(*last) < PULL_COOLDOWN {
                    return Err(refused("try again shortly"));
                }
            }
            times.insert(sale_id.to_string(), now);
            // Evict old entries to bound memory.
            if times.len() > 10_000 {
                times.retain(|_, last| now.saturating_duration_since(*last) < PULL_COOLDOWN);
            }
        }
        let route = Route::of(&sale)?;
        let snapshot = self.build_pull_snapshot(&sale, nonce);
        let sealed = self.seal_sale_reply(route.key, route.drop, &snapshot).map_err(retry)?;
        Ok(json!({
            "ok": true,
            "drop": sale_id,
            "reply": hex(&sealed.to_bytes()),
        }))
    }

    fn build_pull_snapshot(&self, sale: &Value, nonce: &str) -> Value {
        let status = sale["status"].as_str().unwrap_or(AWAITING_PAYMENT);
        let mut snapshot = json!({
            "v": 1,
            "kind": "status",
            "sale": sale["id"],
            "nonce": nonce,
            "status": status,
            "network": sale["network"],
            "itemsSats": sale["itemsSats"],
            "shippingSats": sale["shippingSats"],
            "amountSats": sale["totalSats"],
            "receivedSats": sale["receivedSats"],
            "confirmations": sale["confirmations"],
            "txids": sale["txids"],
            "sellerSawPayment": matches!(status, PAID | SHIPPED | COMPLETED),
            "sellerCompleted": status == COMPLETED,
        });
        if let Some(address) = sale["address"].as_str() {
            snapshot["address"] = json!(address);
        }
        if !sale["quote"].is_null() {
            snapshot["quote"] = sale["quote"].clone();
        }
        if let Some(policy) = sale["returnPolicy"].as_str() {
            snapshot["returnPolicy"] = json!(policy);
        }
        if !sale["returnCase"].is_null() {
            snapshot["returnCase"] = sale["returnCase"].clone();
        }
        if !sale["fulfillment"].is_null() {
            snapshot["fulfillment"] = sale["fulfillment"].clone();
        }
        if !sale["tracking"].is_null() {
            snapshot["tracking"] = sale["tracking"].clone();
        }
        if let Some(payment) = sale.get("payment").filter(|v| !v.is_null()) {
            match payment["rail"].as_str() {
                Some("zec") => crate::zec::add_terms(&mut snapshot, payment),
                Some("xmr") => crate::xmr::add_terms(&mut snapshot, payment),
                Some("ln") => crate::lightning::add_terms(&mut snapshot, payment),
                _ => {}
            }
        }
        if let Some(messages) = sale["messages"].as_array() {
            let mut list = messages.clone();
            snapshot["messages"] = Value::Array(list.clone());
            while body_bytes(&snapshot).len() > tormail::MAX_REPLY_LEN && !list.is_empty() {
                list.remove(0);
                snapshot["messages"] = Value::Array(list.clone());
                snapshot["messagesTrimmed"] = json!(true);
            }
        }
        snapshot
    }

    fn receive_buyer_message(&self, opened: Opened<Vec<u8>>) -> Result<(), MailError> {
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| refused("that message could not be read"))?;
        if body["kind"] == "bid" {
            return self.receive_bid(&opened.sender.0, &body);
        }
        if body["kind"] == "contact" {
            return self.receive_contact(&opened.sender.0, &body);
        }
        let id = body["sale"].as_str().unwrap_or("");
        let sender = hex(&opened.sender.0);
        let Some(sale) = self.sales().get(id).cloned() else {
            return Err(refused("that sale is not on this node"));
        };
        if sale["buyerSender"] != sender.as_str() {
            return Err(refused("that message is not from this sale's buyer"));
        }
        match body["kind"].as_str() {
            Some("complete") => {
                let now = crate::escrow_outbox::now_secs();
                let updated = self
                    .sales()
                    .update(id, |sale| {
                        if sale["status"] == SHIPPED {
                            sale["status"] = json!(COMPLETED);
                            sale["completed"] = json!(now);
                        }
                    })
                    .map_err(retry)?;
                if updated.is_some_and(|sale| sale["status"] == COMPLETED) {
                    self.queue_reply(&Route::of(&sale)?, &json!({ "v": 1, "kind": "completed" })).map_err(retry)?;
                }
                Ok(())
            }
            Some("chat") => {
                let (message_id, text) = crate::messages::received(&body).ok_or_else(|| refused("that message could not be read"))?;
                let now = crate::escrow_outbox::now_secs();
                self.sales()
                    .update(id, |sale| {
                        crate::messages::append(sale, "messages", &message_id, "buyer", &text, now);
                    })
                    .map_err(retry)?;
                Ok(())
            }
            Some("return") => self.receive_return(id, &body),
            Some("sent") => {
                let txid = body["txid"].as_str().filter(|txid| txid.len() == 64 && txid.bytes().all(|byte| byte.is_ascii_hexdigit()));
                let txid = txid.ok_or_else(|| refused("that payment note names no transaction"))?;
                self.sales()
                    .update(id, |sale| {
                        if sale["status"] == AWAITING_PAYMENT || sale["status"] == PAYMENT_SEEN {
                            sale["sentTxid"] = json!(txid);
                        }
                    })
                    .map_err(retry)?;
                Ok(())
            }
            _ => Err(refused("that message is not one this store reads")),
        }
    }

    /// A buyer's return letter: open a case, or name the return shipment.
    /// A letter this sale cannot apply is refused and changes nothing.
    fn receive_return(&self, id: &str, body: &Value) -> Result<(), MailError> {
        let now = crate::escrow_outbox::now_secs();
        let mut problem: Option<&'static str> = None;
        let updated = match body["action"].as_str() {
            Some("request") => {
                let reason = crate::return_case::request_text(body).map_err(refused)?;
                self.sales()
                    .update(id, |sale| {
                        if let Err(err) = crate::return_case::buyer_request(sale, &reason, now) {
                            problem = Some(err);
                        }
                    })
                    .map_err(retry)?
            }
            Some("ship") => {
                let shipment = crate::return_case::shipment_from(body).map_err(refused)?;
                self.sales()
                    .update(id, |sale| {
                        if let Err(err) = crate::return_case::mark_shipped(sale, &shipment, now) {
                            problem = Some(err);
                        }
                    })
                    .map_err(retry)?
            }
            _ => return Err(refused("that return could not be read")),
        };
        if updated.is_none() {
            return Err(refused("that sale is not on this node"));
        }
        match problem {
            Some(err) => Err(refused(err)),
            None => Ok(()),
        }
    }

    /// `POST /sales/{id}/return`: accept or decline the buyer's return.
    pub(crate) fn decide_return(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let (accept, note) = crate::return_case::decision_from(body).map_err(StoreError::invalid)?;
        let sale = self.sales().get(id).cloned().ok_or(StoreError::NotFound)?;
        let now = crate::escrow_outbox::now_secs();
        let mut trial = sale.clone();
        crate::return_case::decide(&mut trial, accept, &note, now).map_err(StoreError::invalid)?;
        let reply = json!({
            "v": 1,
            "kind": "return",
            "action": if accept { "accept" } else { "decline" },
            "text": note,
        });
        self.tell_buyer(&sale, &reply).map_err(StoreError::invalid)?;
        let mut problem: Option<&'static str> = None;
        let updated = self
            .sales()
            .update(id, |sale| {
                if let Err(err) = crate::return_case::decide(sale, accept, &note, now) {
                    problem = Some(err);
                }
            })
            .map_err(StoreError::Failed)?
            .ok_or(StoreError::NotFound)?;
        if let Some(err) = problem {
            return Err(StoreError::invalid(err));
        }
        Ok(public_sale(&updated))
    }

    /// `POST /sales/{id}/fulfill`: mark a paid sale shipped and tell the buyer.
    pub(crate) fn fulfill_sale(&self, id: &str, body: &Value) -> Result<Value, StoreError> {
        let field = |key: &str, max: usize| -> Result<String, StoreError> {
            let text = body[key].as_str().unwrap_or("").trim().to_string();
            if text.len() > max || text.contains('\0') {
                return Err(StoreError::Invalid(format!("The {key} is too long.")));
            }
            Ok(text)
        };
        let fulfillment = json!({
            "carrier": field("carrier", 60)?,
            "tracking": field("tracking", 100)?,
            "note": field("note", 1000)?,
            "content": field("content", 4000)?,
            "at": crate::escrow_outbox::now_secs(),
        });
        let sale = self.sales().get(id).cloned().ok_or(StoreError::NotFound)?;
        let physical = sale["lines"].as_array().into_iter().flatten().any(|line| line["kind"] == "physical");
        if sale["status"] == SHIPPED || sale["status"] == COMPLETED {
            return Ok(public_sale(&sale));
        }
        if sale["status"] != PAID {
            return Err(StoreError::Invalid("Fulfill a sale once it is paid.".into()));
        }
        if !physical && fulfillment["content"] == "" && fulfillment["note"] == "" {
            return Err(StoreError::Invalid("Add the download link, code, or note the buyer receives.".into()));
        }
        let reply = json!({
            "v": 1,
            "kind": "shipped",
            "carrier": fulfillment["carrier"],
            "tracking": fulfillment["tracking"],
            "note": fulfillment["note"],
            "content": fulfillment["content"],
        });
        let tracking = physical.then(|| {
            crate::tracking::initial_tracking(
                fulfillment["carrier"].as_str().unwrap_or(""),
                fulfillment["tracking"].as_str().unwrap_or(""),
                crate::escrow_outbox::now_secs(),
            )
        });
        let reply = self.with_pending_receipts(&sale, reply);
        let route = Route::of(&sale).map_err(|err| StoreError::Failed(format!("{err:?}")))?;
        self.queue_reply(&route, &reply).map_err(StoreError::Failed)?;
        let updated = self
            .sales()
            .update(id, |sale| {
                sale["status"] = json!(SHIPPED);
                sale["fulfillment"] = fulfillment;
                if let Some(tracking) = tracking {
                    sale["tracking"] = tracking;
                }
            })
            .map_err(StoreError::Failed)?
            .ok_or(StoreError::NotFound)?;
        Ok(public_sale(&updated))
    }

    /// One watcher round over open sales. Returns how many changed.
    pub(crate) fn watch_sales(&self, tip: u32) -> usize {
        let open: Vec<(String, String, u64)> = self
            .sales()
            .list()
            .filter(|sale| sale["status"] == AWAITING_PAYMENT || sale["status"] == PAYMENT_SEEN)
            .filter_map(|sale| {
                Some((sale["id"].as_str()?.to_string(), sale["address"].as_str()?.to_string(), sale["totalSats"].as_u64()?))
            })
            .collect();
        let mut changed = 0;
        for (id, address, due) in open {
            let Ok(address) = crate::network::address_for(&address, self.network) else { continue };
            let txs = match self.chain.address_txs(&address) {
                Ok(txs) => txs,
                Err(err) => {
                    eprintln!("ob-node: chain query for sale {id}: {err}");
                    continue;
                }
            };
            let seen = direct_status(due, &txs, tip, self.settings.confirmations);
            let (mut moved, mut became_paid) = (false, false);
            let result = self.sales().update(&id, |sale| {
                let before = sale.clone();
                became_paid = seen.status == PAID && sale["status"] != PAID;
                sale["status"] = json!(seen.status);
                sale["receivedSats"] = json!(seen.received_sats);
                sale["confirmations"] = json!(seen.confirmations);
                sale["txids"] = json!(seen.txids);
                moved = *sale != before;
            });
            match result {
                Ok(Some(sale)) => {
                    changed += usize::from(moved);
                    if became_paid {
                        let reply = self.paid_reply(&sale);
                        if let Err(err) = Route::of(&sale).map_err(|err| format!("{err:?}")).and_then(|route| self.queue_reply(&route, &reply)) {
                            eprintln!("ob-node: sale {id}: {err}");
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => eprintln!("ob-node: sale {id}: {err}"),
            }
        }
        changed
    }

    /// Queue `reply` for the buyer of `sale`.
    pub(crate) fn tell_buyer(&self, sale: &Value, reply: &Value) -> Result<(), String> {
        let route = Route::of(sale).map_err(|err| format!("{err:?}"))?;
        self.queue_reply(&route, reply)
    }

    /// Seal `body` to the buyer's reply key and queue it for the buyer's node.
    /// Pull-only orders (`route.url.is_empty()`) skip push queueing because
    /// the buyer fetches sealed snapshots via `POST /mail/pull`.
    fn queue_reply(&self, route: &Route, body: &Value) -> Result<(), String> {
        if route.url.is_empty() {
            return Ok(());
        }
        self.queue_buyer_reply(&route.url, route.key, route.drop, body)
    }

    pub(crate) fn seal_sale_reply(&self, key: [u8; 32], drop: [u8; 16], body: &Value) -> Result<tormail::SealedReply, String> {
        let bytes = body_bytes(body);
        let key = ReplyPublic::from_bytes(key).map_err(|err| err.to_string())?;
        let from = self.sale_letter_key(&drop)?;
        seal_reply(&bytes, &key, &from).map_err(|err| err.to_string())
    }

    /// Seal `body` to a bid or order reply key and queue it for that buyer's node.
    pub(crate) fn queue_buyer_reply(&self, url: &str, key: [u8; 32], drop: [u8; 16], body: &Value) -> Result<(), String> {
        let bytes = body_bytes(body);
        let sealed = self.seal_sale_reply(key, drop, body)?;
        let frame = reply_frame(DropId(drop), &sealed);
        let url = format!("{url}{REPLY_MAIL}");
        self.order_outbox.queue(mail_key(&url, &drop, &bytes), &url, &frame)
    }
}

pub(crate) struct Priced {
    pub lines: Vec<Value>,
    pub items: u64,
    pub shipping: u64,
    pub digital: bool,
    pub physical: bool,
    /// The fiat quote of a version 3 order, else null.
    pub quote: Value,
    pub checked: Option<crate::order_terms::Checked>,
    /// The return policy this order was sold under, when one allows a return.
    pub return_policy: Option<String>,
}

/// Where replies in one sale go.
struct Route {
    url: String,
    key: [u8; 32],
    drop: [u8; 16],
}

impl Route {
    fn of(sale: &Value) -> Result<Self, MailError> {
        let bad = || retry("a stored sale has no reply route");
        let key = crate::stores::unhex32(sale["replyKey"].as_str().unwrap_or("")).ok_or_else(bad)?;
        let drop: [u8; 16] = crate::escrow_mail::parse_id(sale["id"].as_str().unwrap_or("")).ok_or_else(bad)?;
        Ok(Self { url: sale["replyUrl"].as_str().unwrap_or("").to_string(), key, drop })
    }
}
