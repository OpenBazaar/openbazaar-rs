//! The buyer's side of an order to a store on another node.
//!
//! Checkout seals the letter as for any store, with this node's base URL as
//! the `replyUrl` option, and hands it to the order outbox for the URL the
//! store is followed at. The order starts as "Awaiting seller": only the
//! seller prices it (its rate, its stock) and names the address.
//!
//! Replies arrive at `POST /mail/reply` ([`crate::order_mail`]) and are
//! filed by drop id. The first reply pins the seller's per-sale key; later
//! ones must match. Terms set the payment, unless they ask for more than
//! checkout showed (exactly that for a sats price; a quarter more for a
//! fiat price, which moves with the rate), in which case the order stops.
//! From there the chain watcher moves the order to Paid as for any direct
//! order (a Zcash or Monero order follows the seller's replies instead, see
//! `zec/sale.rs` and `xmr/sale.rs`), a "shipped" reply carries the tracking or the digital goods, and
//! `POST /orders/{id}/complete` tells the seller it arrived.

use std::time::Duration;

use openbazaar_protocol::Listing;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Value};
use tormail::{open_reply, seal_message, ReplyDelivery, SealedLetter, SealedReply};

use crate::buyer_light::{build_pull_request, open_pull_response};
use crate::escrow_mail::{hex, unhex};
use crate::escrow_outbox::Sent;
use crate::order_mail::{body_bytes, mail_key, MailError, PULL_MAIL, STORE_MAIL};
use crate::orders::package_shipping;
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, COMPLETED, DECLINED, PAID, PAYMENT_SEEN, SHIPPED};
use crate::{Node, OrderError};

fn refused(message: impl Into<String>) -> MailError {
    MailError::Refused(message.into())
}

fn retry(message: impl std::fmt::Display) -> MailError {
    MailError::Retry(message.to_string())
}

fn order_err(err: crate::OrderError) -> String {
    match err {
        crate::OrderError::Invalid(message) => message.to_string(),
        crate::OrderError::Failed(detail) => format!("The seller's node could not take the return. {detail}"),
        err => err.to_string(),
    }
}

/// The most a seller may ask: what checkout showed, or a quarter more for a fiat price.
fn ceiling(estimate: u64, fiat: bool) -> u64 {
    if fiat {
        estimate.saturating_add(estimate / 4)
    } else {
        estimate
    }
}

impl Node {
    /// The followed URL when `listing` is from a store on another node.
    /// Refuses the own store. Direct bitcoin, Zcash, or Monero orders go
    /// over the order mail to that URL. Escrow is not a remote order: it
    /// uses `--escrow-vendor` on this node, so a followed testnet store that
    /// offers moderated listings can be checked out with `settlement: escrow`.
    pub(crate) fn remote_package(&self, listing: &Listing, direct: bool) -> Result<Option<String>, OrderError> {
        if listing.store_id == self.own_store {
            return Err(OrderError::Invalid("That is your own store."));
        }
        let followed = self.replicas().url_of(&listing.store_id).map(str::to_string);
        let Some(url) = followed.or_else(|| self.directory_sale_url(&listing.store_id)) else {
            return Ok(None);
        };
        if !direct {
            return Ok(None);
        }
        Ok(Some(url))
    }

    /// Items and shipping in sats at this node's rate now: what checkout shows.
    pub(crate) fn remote_estimate(&self, lines: &[(&Listing, u32)]) -> Result<(u64, u64), OrderError> {
        let priced: Vec<Listing> = lines.iter().map(|(listing, _)| self.priced_now(listing)).collect();
        let refs: Vec<(&Listing, u32)> = priced.iter().zip(lines).map(|(listing, (_, quantity))| (listing, *quantity)).collect();
        package_shipping(&refs)
    }

    /// What the order record keeps about a remote order.
    pub(crate) fn remote_record(
        &self,
        url: &str,
        checkout_id: &str,
        listing: &Listing,
        lines: &[(&Listing, u32)],
        estimate: u64,
    ) -> Result<Value, OrderError> {
        let (_, drop) = self.order_reply_secret(checkout_id, listing.store_id.as_bytes()).map_err(OrderError::Failed)?;
        let fiat = lines.iter().any(|(listing, _)| listing.details.as_ref().is_some_and(|details| details.price.is_some()));
        let reply_mode = if self.sale_reply_url().is_some() { "push" } else { "pull" };
        Ok(json!({
            "url": url,
            "drop": hex(&drop.0),
            "estimateSats": estimate,
            "fiat": fiat,
            "delivered": false,
            "replyMode": reply_mode,
        }))
    }

    /// Queue the sealed order for the seller's node and try it once.
    /// In pull mode (when this node has no 24/7 reply URL), tries a direct
    /// POST first so an immediate sealed `terms` reply is applied right away.
    pub(crate) fn deliver_remote_order(&self, id: &str, url: &str, checkout_id: &str, listing: &Listing, letter: &SealedLetter) {
        let target = format!("{url}{STORE_MAIL}");
        let conversation = [checkout_id.as_bytes(), listing.store_id.as_bytes()].concat();
        let bytes = letter.to_bytes();
        if self.sale_reply_url().is_none() {
            if let Ok(answer) = crate::peer_http::call(
                &self.peers,
                "POST",
                &target,
                Some(&bytes),
                Some("application/octet-stream"),
                Duration::from_secs(15),
                64 * 1024,
            ) {
                if (200..300).contains(&answer.status) {
                    let parsed = serde_json::from_slice::<Value>(&answer.body).ok();
                    let immediate = parsed
                        .as_ref()
                        .and_then(|v| v["reply"].as_str())
                        .and_then(|hex_str| unhex(hex_str).ok());
                    if let Some(reply_bytes) = immediate {
                        let _ = self.apply_sealed_reply_bytes(id, &reply_bytes);
                        return;
                    }
                    let _ = self.orders().update(id, |order| {
                        let remote = order.remote.get_or_insert_with(|| json!({}));
                        remote["delivered"] = json!(true);
                    });
                    return;
                }
            }
        }
        let result = self.order_outbox.send(mail_key(&target, &conversation, &bytes), &target, &bytes);
        let update = |order: &mut crate::buyer_orders::BuyerOrder| {
            let remote = order.remote.get_or_insert_with(|| json!({}));
            match &result {
                Ok(Sent::Delivered) => remote["delivered"] = json!(true),
                Ok(Sent::Queued(err)) => remote["error"] = json!(format!("The seller's node is offline; the order will be sent when it is back. ({err})")),
                Err(err) => remote["error"] = json!(format!("The seller's node refused the order. ({err})")),
            }
        };
        if let Err(err) = self.orders().update(id, update) {
            eprintln!("ob-node: order {id}: {err}");
        }
    }

    fn apply_sealed_reply_bytes(&self, id: &str, reply_bytes: &[u8]) -> Result<(), String> {
        let order = self.orders().get(id).cloned().ok_or_else(|| "that order is not on this node".to_string())?;
        let store = crate::stores::unhex32(&order.store_id).ok_or_else(|| "the order names no store key".to_string())?;
        let (secret, _) = self.order_reply_secret(&order.checkout_id, &store)?;
        let sealed = SealedReply::from_bytes(reply_bytes).map_err(|_| "that reply is malformed".to_string())?;
        let opened = open_reply(&sealed, &secret).map_err(|_| "that reply does not open".to_string())?;
        let sender = hex(&opened.sender.0);
        let pinned = order.remote.as_ref().and_then(|remote| remote["seller"].as_str().map(str::to_string));
        if pinned.as_ref().is_some_and(|pinned| *pinned != sender) {
            return Err("that reply is not from this order's seller".into());
        }
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| "that reply could not be read".to_string())?;
        crate::buyer_light::check_pull_text(&body)?;
        let network = self.network;
        let review_key = self.order_review_key(&order.checkout_id, &store).ok();
        self.orders()
            .update(&order.id, |order| {
                let remote = order.remote.get_or_insert_with(|| json!({}));
                remote["seller"] = json!(sender);
                remote["delivered"] = json!(true);
                if let Some(map) = remote.as_object_mut() {
                    map.remove("error");
                }
                apply_reply(order, &body, network);
                crate::checkout_terms::keep_receipts(order, &body, review_key);
            })
            .map(|_| ())
    }

    /// `POST /orders/{id}/pull`: pull sealed status/tracking/chat/return updates
    /// directly from the seller's `POST /mail/pull` endpoint using this order's
    /// derived key. Works without a 24/7 buyer Tor mailbox.
    pub(crate) fn pull_remote_order(&self, id: &str) -> Result<Value, OrderError> {
        let order = self.orders().get(id).cloned().ok_or(OrderError::Invalid("That order is not on this node."))?;
        let Some(remote) = &order.remote else {
            return Err(OrderError::Invalid("Only an order to another node's store can pull updates."));
        };
        let url = remote["url"].as_str().filter(|u| !u.is_empty()).ok_or(OrderError::Invalid("This order has no seller URL."))?;
        let store = crate::stores::unhex32(&order.store_id).ok_or_else(|| OrderError::Failed("the order names no store key".into()))?;
        let store_mailbox = {
            let catalog = self.catalog();
            let record = catalog.stores.iter().find(|store_record| store_record.id.as_bytes() == &store);
            record.map(crate::stores::mailbox_key).transpose().map_err(OrderError::Failed)?
        }
        .ok_or(OrderError::Invalid("This node no longer has the store's record."))?;
        let from = self.order_letter_key(&order.checkout_id, &store).map_err(OrderError::Failed)?;
        let (reply_secret, drop) = self.order_reply_secret(&order.checkout_id, &store).map_err(OrderError::Failed)?;
        let mut nonce = [0u8; 16];
        StdRng::from_os_rng().fill_bytes(&mut nonce);
        let letter = build_pull_request(&drop.0, &from, &store_mailbox, &nonce).map_err(OrderError::Failed)?;
        let target = format!("{url}{PULL_MAIL}");
        let answer = crate::peer_http::call(
            &self.peers,
            "POST",
            &target,
            Some(&letter.to_bytes()),
            Some("application/octet-stream"),
            Duration::from_secs(15),
            64 * 1024,
        )
        .map_err(|err| OrderError::Failed(format!("Could not reach the seller's node. ({err})")))?;
        if !(200..300).contains(&answer.status) {
            return Err(OrderError::Failed(format!(
                "The seller's node refused the status request (HTTP {}).",
                answer.status
            )));
        }
        let parsed: Value = serde_json::from_slice(&answer.body)
            .map_err(|_| OrderError::Failed("The seller's status answer could not be read.".into()))?;
        let reply_hex = parsed["reply"]
            .as_str()
            .ok_or_else(|| OrderError::Failed("The seller's status answer carried no sealed reply.".into()))?;
        let reply_bytes = unhex(reply_hex).map_err(OrderError::Failed)?;
        let pinned = remote["seller"].as_str();
        let (sender, snapshot) = open_pull_response(&reply_bytes, &reply_secret, &nonce, pinned).map_err(OrderError::Failed)?;
        let network = self.network;
        self.orders()
            .update(&order.id, |order| {
                let remote = order.remote.get_or_insert_with(|| json!({}));
                remote["seller"] = json!(sender);
                remote["delivered"] = json!(true);
                if let Some(map) = remote.as_object_mut() {
                    map.remove("error");
                }
                apply_pull_status(order, &snapshot, network);
            })
            .map_err(OrderError::Failed)?;
        Ok(self.orders().get_value(id).unwrap_or_default())
    }

    /// `POST /mail/reply`: a seller's reply to one of this node's orders.
    pub(crate) fn receive_reply(&self, bytes: &[u8]) -> Result<(), MailError> {
        let (drop, sealed) = ReplyDelivery::from_frame(bytes).map_err(|_| refused("that is not a reply frame"))?;
        let drop_hex = hex(&drop.0);
        let order = self
            .orders()
            .on_network()
            .find(|order| order.remote.as_ref().is_some_and(|remote| remote["drop"] == drop_hex.as_str()))
            .cloned()
            .ok_or_else(|| refused("no order here waits on that reply"))?;
        let store = crate::stores::unhex32(&order.store_id).ok_or_else(|| retry("the order names no store key"))?;
        let (secret, _) = self.order_reply_secret(&order.checkout_id, &store).map_err(retry)?;
        let opened = open_reply(&sealed, &secret).map_err(|_| refused("that reply does not open"))?;
        let sender = hex(&opened.sender.0);
        let pinned = order.remote.as_ref().and_then(|remote| remote["seller"].as_str().map(str::to_string));
        if pinned.as_ref().is_some_and(|pinned| *pinned != sender) {
            return Err(refused("that reply is not from this order's seller"));
        }
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| refused("that reply could not be read"))?;
        let network = self.network;
        let review_key = self.order_review_key(&order.checkout_id, &store).ok();
        self.orders()
            .update(&order.id, |order| {
                let remote = order.remote.get_or_insert_with(|| json!({}));
                remote["seller"] = json!(sender);
                remote["delivered"] = json!(true);
                if let Some(map) = remote.as_object_mut() {
                    map.remove("error");
                }
                apply_reply(order, &body, network);
                crate::checkout_terms::keep_receipts(order, &body, review_key);
            })
            .map_err(retry)?;
        Ok(())
    }

    /// `POST /orders/{id}/message`: the buyer writes to this order's seller.
    pub(crate) fn send_order_message(&self, id: &str, body: &Value) -> Result<Value, OrderError> {
        let order = self.orders().get(id).cloned().ok_or(OrderError::Invalid("That order is not on this node."))?;
        let Some(remote) = &order.remote else {
            return Err(OrderError::Invalid("Only an order to another node's store can take a message here."));
        };
        let text = crate::messages::clean_text(body).map_err(OrderError::Failed)?;
        let message_id = crate::messages::message_id();
        let (key, target, letter) = self.seller_letter(&order, remote, &json!({ "kind": "chat", "id": message_id, "text": text }))?;
        self.order_outbox
            .send(key, &target, &letter)
            .map_err(|err| OrderError::Failed(format!("The seller's node refused the message. ({err})")))?;
        let now = crate::escrow_outbox::now_secs();
        self.orders()
            .update(id, |order| {
                if let Some(remote) = order.remote.as_mut() {
                    crate::messages::append(remote, "messages", &message_id, "buyer", &text, now);
                }
            })
            .map_err(OrderError::Failed)?;
        Ok(self.orders().get_value(id).unwrap_or_default())
    }

    /// `POST /orders/{id}/return`: the buyer opens a return on an order that
    /// was sold with a return policy, once the seller has shipped it.
    pub(crate) fn request_return(&self, id: &str, body: &Value) -> Result<Value, String> {
        let order = self.orders().get(id).cloned().ok_or_else(|| "That order is not on this node.".to_string())?;
        let remote = order.remote.as_ref().ok_or_else(|| "Only an order to another node's store can be returned here.".to_string())?;
        if remote["returnPolicy"].as_str().map(str::trim).unwrap_or("").is_empty() {
            return Err("This order has no return policy.".to_string());
        }
        if order.status != SHIPPED && order.status != COMPLETED {
            return Err("Open a return once the seller has shipped the order.".to_string());
        }
        let text = crate::return_case::request_text(body)?;
        let (key, target, letter) = self
            .seller_letter(&order, remote, &json!({ "kind": "return", "action": "request", "text": text.clone() }))
            .map_err(order_err)?;
        self.order_outbox
            .send(key, &target, &letter)
            .map_err(|err| format!("The seller's node refused the return. ({err})"))?;
        let now = crate::escrow_outbox::now_secs();
        let mut problem: Option<&'static str> = None;
        self.orders()
            .update(id, |order| {
                if let Some(remote) = order.remote.as_mut() {
                    if let Err(err) = crate::return_case::record_request(remote, &text, now) {
                        problem = Some(err);
                    }
                }
            })
            .map_err(|err| format!("The return could not be saved. ({err})"))?;
        match problem {
            Some(err) => Err(err.to_string()),
            None => Ok(self.orders().get_value(id).unwrap_or_default()),
        }
    }

    /// `POST /orders/{id}/return/shipment`: the buyer names the return shipment
    /// after the seller has accepted. The number is a reference, not a lookup.
    pub(crate) fn ship_return(&self, id: &str, body: &Value) -> Result<Value, String> {
        let order = self.orders().get(id).cloned().ok_or_else(|| "That order is not on this node.".to_string())?;
        let remote = order.remote.as_ref().ok_or_else(|| "Only an order to another node's store can be returned here.".to_string())?;
        let state = remote["returnCase"]["state"].as_str();
        if state != Some(crate::return_case::STATE_ACCEPTED) && state != Some(crate::return_case::STATE_SHIPPED) {
            return Err("Ship a return once the seller has accepted it.".to_string());
        }
        let shipment = crate::return_case::shipment_from(body)?;
        let (key, target, letter) = self
            .seller_letter(
                &order,
                remote,
                &json!({
                    "kind": "return",
                    "action": "ship",
                    "carrier": shipment.carrier.clone(),
                    "tracking": shipment.tracking.clone(),
                    "note": shipment.note.clone(),
                }),
            )
            .map_err(order_err)?;
        self.order_outbox
            .send(key, &target, &letter)
            .map_err(|err| format!("The seller's node refused the return shipment. ({err})"))?;
        let now = crate::escrow_outbox::now_secs();
        let mut problem: Option<&'static str> = None;
        self.orders()
            .update(id, |order| {
                if let Some(remote) = order.remote.as_mut() {
                    if let Err(err) = crate::return_case::mark_shipped(remote, &shipment, now) {
                        problem = Some(err);
                    }
                }
            })
            .map_err(|err| format!("The return shipment could not be saved. ({err})"))?;
        match problem {
            Some(err) => Err(err.to_string()),
            None => Ok(self.orders().get_value(id).unwrap_or_default()),
        }
    }

    /// `POST /orders/{id}/complete`: the goods arrived. Tells the seller.
    pub(crate) fn complete_order(&self, id: &str) -> Result<Value, OrderError> {
        let order = self.orders().get(id).cloned().ok_or(OrderError::Invalid("That order is not on this node."))?;
        let Some(remote) = &order.remote else {
            return Err(OrderError::Invalid("Only an order to another node's store is completed here."));
        };
        if order.status == COMPLETED {
            return Ok(self.orders().get_value(id).unwrap_or_default());
        }
        if order.status != SHIPPED {
            return Err(OrderError::Invalid("Complete an order once the seller has shipped it."));
        }
        let (key, target, letter) = self.seller_letter(&order, remote, &json!({ "kind": "complete" }))?;
        self.order_outbox
            .send(key, &target, &letter)
            .map_err(|err| OrderError::Failed(format!("The seller's node refused the message. ({err})")))?;
        self.orders()
            .update(id, |order| {
                order.status = COMPLETED.to_string();
                if let Some(remote) = order.remote.as_mut() {
                    remote["completed"] = json!(crate::escrow_outbox::now_secs());
                }
            })
            .map_err(OrderError::Failed)?;
        Ok(self.orders().get_value(id).unwrap_or_default())
    }
}

impl Node {
    /// Queue a "sent" letter so the seller sees the payment before a block
    /// does: compact block filters show a seller nothing unconfirmed.
    pub(crate) fn tell_seller_sent(&self, id: &str, txid: &str) -> Result<(), OrderError> {
        let order = self.orders().get(id).cloned().ok_or(OrderError::Invalid("That order is not on this node."))?;
        let Some(remote) = &order.remote else { return Ok(()) };
        let (key, target, letter) = self.seller_letter(&order, remote, &json!({ "kind": "sent", "txid": txid }))?;
        self.order_outbox.queue(key, &target, &letter).map_err(OrderError::Failed)
    }

    /// `message`, addressed to this order's sale and sealed to the store's mailbox key.
    fn seller_letter(
        &self,
        order: &crate::buyer_orders::BuyerOrder,
        remote: &Value,
        message: &Value,
    ) -> Result<([u8; 32], String, Vec<u8>), OrderError> {
        let store = crate::stores::unhex32(&order.store_id).ok_or(OrderError::Failed("the order names no store key".into()))?;
        let mail_key_bytes = {
            let catalog = self.catalog();
            let record = catalog.stores.iter().find(|store_record| store_record.id.as_bytes() == &store);
            record.map(crate::stores::mailbox_key).transpose().map_err(OrderError::Failed)?
        }
        .ok_or(OrderError::Invalid("This node no longer has the store's record."))?;
        let from = self.order_letter_key(&order.checkout_id, &store).map_err(OrderError::Failed)?;
        let mut full = json!({ "v": 1, "sale": remote["drop"] });
        if let (Some(map), Some(extra)) = (full.as_object_mut(), message.as_object()) {
            map.extend(extra.clone());
        }
        let body = body_bytes(&full);
        let letter = seal_message(&body, &mail_key_bytes, &from).map_err(|err| OrderError::Failed(err.to_string()))?;
        let target = format!("{}{STORE_MAIL}", remote["url"].as_str().unwrap_or_default());
        let conversation = [order.checkout_id.as_bytes(), &store[..]].concat();
        Ok((mail_key(&target, &conversation, &body), target, letter.to_bytes()))
    }
}

/// Apply one seller reply to the order. Each kind is idempotent and only
/// moves the order forward.
pub(crate) fn apply_reply(order: &mut crate::buyer_orders::BuyerOrder, body: &Value, network: crate::NetworkMode) {
    let remote = order.remote.clone().unwrap_or_default();
    crate::zec::follow_seller(order, &body["zec"]);
    crate::xmr::follow_seller(order, &body["xmr"]);
    match body["kind"].as_str() {
        Some("terms") if order.status == AWAITING_SELLER => {
            let amount = body["amountSats"].as_u64().unwrap_or(0);
            let estimate = remote["estimateSats"].as_u64().unwrap_or(0);
            let address = body["address"].as_str().unwrap_or("");
            let limit = ceiling(estimate, remote["fiat"].as_bool().unwrap_or(false));
            let problem = crate::checkout_terms::quote_problem(&remote, body);
            let payment = match remote["rail"].as_str() {
                Some("zec") => crate::zec::payment_from_terms(body, network.zcash()),
                Some("xmr") => crate::xmr::payment_from_terms(body, network.monero()),
                Some("ln") => crate::lightning::payment_from_terms(body, network),
                _ => crate::network::address_for(address, network).map_err(|_| "The seller's payment address is not valid here.".to_string()).map(|address| {
                    json!({
                        "network": network.as_str(),
                        "onchain": address.to_string(),
                        "address": address.to_string(),
                        "amountSats": amount,
                        "bip21": crate::network::bip21(&address, bitcoin::Amount::from_sat(amount)),
                        "watched": ["onchain"],
                    })
                }),
            };
            let accepted = match payment {
                Err(invalid) => Err(invalid),
                Ok(_) if problem.is_some() => Err(problem.unwrap_or_default()),
                Ok(_) if amount == 0 || amount > limit => {
                    Err(format!("The seller asked {amount} sats; checkout showed {estimate}. Nothing was paid."))
                }
                Ok(payment) => Ok(payment),
            };
            match accepted {
                Ok(payment) => {
                    order.items_sats = body["itemsSats"].as_u64().unwrap_or(order.items_sats);
                    order.shipping_sats = body["shippingSats"].as_u64().unwrap_or(order.shipping_sats);
                    order.total_sats = amount;
                    order.payment = Some(payment);
                    order.status = AWAITING_PAYMENT.to_string();
                    if let Some(remote) = order.remote.as_mut().filter(|_| !body["quote"].is_null()) {
                        remote["sellerQuote"] = body["quote"].clone();
                    }
                    if let Some(policy) = crate::return_case::policy_from_reply(&body["returnPolicy"]) {
                        if let Some(remote) = order.remote.as_mut() {
                            remote["returnPolicy"] = json!(policy);
                        }
                    }
                }
                Err(problem) => {
                    order.status = DECLINED.to_string();
                    if let Some(remote) = order.remote.as_mut() {
                        remote["reason"] = json!(problem);
                    }
                }
            }
        }
        Some("declined") if order.status == AWAITING_SELLER => {
            order.status = DECLINED.to_string();
            if let Some(remote) = order.remote.as_mut() {
                remote["reason"] = json!(body["reason"].as_str().unwrap_or("The seller declined the order."));
            }
        }
        Some("paid") => {
            if let Some(remote) = order.remote.as_mut() {
                remote["sellerSawPayment"] = json!(true);
            }
            if order.status == AWAITING_PAYMENT || order.status == PAYMENT_SEEN {
                order.status = crate::watch::PAID.to_string();
            }
        }
        Some("shipped") if order.status != COMPLETED && order.status != DECLINED && order.payment.is_some() => {
            order.status = SHIPPED.to_string();
            if let Some(remote) = order.remote.as_mut() {
                remote["fulfillment"] = json!({
                    "carrier": body["carrier"],
                    "tracking": body["tracking"],
                    "note": body["note"],
                    "content": body["content"],
                });
            }
        }
        Some("tracking") if order.status == SHIPPED => {
            if let Some(remote) = order.remote.as_mut() {
                let fulfillment = remote["fulfillment"].clone();
                if let Some(update) = crate::tracking::tracking_update_from_body(
                    body,
                    fulfillment["carrier"].as_str().unwrap_or(""),
                    fulfillment["tracking"].as_str().unwrap_or(""),
                    crate::escrow_outbox::now_secs(),
                ) {
                    remote["tracking"] = update;
                }
            }
        }
        Some("completed") => {
            if let Some(remote) = order.remote.as_mut() {
                remote["sellerCompleted"] = json!(true);
            }
        }
        Some("chat") => {
            if let Some(remote) = order.remote.as_mut() {
                if let Some((message_id, text)) = crate::messages::received(body) {
                    crate::messages::append(remote, "messages", &message_id, "seller", &text, crate::escrow_outbox::now_secs());
                }
            }
        }
        Some("return") => {
            if let Some(remote) = order.remote.as_mut() {
                let _ = crate::return_case::apply_seller_reply(remote, body, crate::escrow_outbox::now_secs());
            }
        }
        _ => {}
    }
}

/// Apply one authenticated pull status snapshot from `POST /mail/pull`.
/// Reuses the exact same price-ceiling and network-address checks as push
/// replies and only moves the order forward.
pub(crate) fn apply_pull_status(order: &mut crate::buyer_orders::BuyerOrder, body: &Value, network: crate::NetworkMode) {
    if order.status == AWAITING_SELLER {
        let mut terms = body.clone();
        terms["kind"] = json!("terms");
        apply_reply(order, &terms, network);
        if order.status == DECLINED {
            return;
        }
    }
    let status = body["status"].as_str().unwrap_or("");
    if body["sellerSawPayment"].as_bool() == Some(true) || matches!(status, PAID | SHIPPED | COMPLETED) {
        apply_reply(order, &json!({ "kind": "paid" }), network);
    }
    if let Some(fulfillment) = body.get("fulfillment").filter(|v| v.is_object()) {
        let mut shipped = fulfillment.clone();
        shipped["kind"] = json!("shipped");
        apply_reply(order, &shipped, network);
    }
    if let Some(tracking) = body.get("tracking").filter(|v| v.is_object()) {
        if order.status == SHIPPED {
            if let Some(remote) = order.remote.as_mut() {
                remote["tracking"] = tracking.clone();
            }
        }
    }
    if body["sellerCompleted"].as_bool() == Some(true) || status == COMPLETED {
        apply_reply(order, &json!({ "kind": "completed" }), network);
    }
    if let Some(policy) = crate::return_case::policy_from_reply(&body["returnPolicy"]) {
        if let Some(remote) = order.remote.as_mut() {
            if remote.get("returnPolicy").and_then(Value::as_str).unwrap_or("").is_empty() {
                remote["returnPolicy"] = json!(policy);
            }
        }
    }
    if let Some(return_case) = body.get("returnCase").filter(|v| v.is_object()) {
        if let Some(remote) = order.remote.as_mut() {
            let state = return_case["state"].as_str();
            if matches!(state, Some("accepted") | Some("declined")) {
                let action = if state == Some("accepted") { "accept" } else { "decline" };
                let text = return_case["decision"].as_str().unwrap_or("");
                let _ = crate::return_case::apply_seller_reply(
                    remote,
                    &json!({ "kind": "return", "action": action, "text": text }),
                    crate::escrow_outbox::now_secs(),
                );
            }
        }
    }
    if let Some(messages) = body["messages"].as_array() {
        if let Some(remote) = order.remote.as_mut() {
            for message in messages {
                let from = message["from"].as_str();
                if matches!(from, Some("buyer") | Some("seller")) {
                    if let Some((message_id, text)) = crate::messages::received(message) {
                        let at = message["at"].as_u64().unwrap_or_else(crate::escrow_outbox::now_secs);
                        crate::messages::append(remote, "messages", &message_id, from.unwrap_or("seller"), &text, at);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fiat_price_may_move_a_quarter_a_sats_price_not_at_all() {
        assert_eq!(ceiling(10_000, false), 10_000);
        assert_eq!(ceiling(10_000, true), 12_500);
    }

    #[test]
    fn a_bad_return_reply_does_not_change_the_order() {
        let mut order = crate::buyer_orders::BuyerOrder {
            id: "order".into(),
            checkout_id: "checkout".into(),
            created: 1,
            seller: "Brook and Pine".into(),
            store_id: "ab".repeat(32),
            lines: vec![],
            items_sats: 25_000,
            shipping_sats: 800,
            total_sats: 25_800,
            settlement: "direct".into(),
            payment: Some(json!({ "address": "bcrt1qexample", "amountSats": 25_800 })),
            escrow: None,
            address_label: String::new(),
            status: crate::watch::SHIPPED.to_string(),
            network: "testnet4".into(),
            received_sats: 25_800,
            confirmations: 1,
            txids: vec![],
            pay_txid: None,
            paying: false,
            pay_error: None,
            remote: Some(json!({
                "returnPolicy": "Unused goods can come back within 30 days.",
                "returnCase": { "state": "requested", "reason": "The glaze is crazed.", "requestedAt": 1 }
            })),
        };
        let network = crate::NetworkMode::Testnet4;
        apply_reply(&mut order, &json!({ "kind": "return", "action": "release", "text": "send the coins back" }), network);
        apply_reply(&mut order, &json!({ "kind": "return", "action": "accept", "text": "mail me at a@b.co" }), network);
        assert_eq!(order.status, crate::watch::SHIPPED);
        assert_eq!(order.total_sats, 25_800);
        assert_eq!(order.payment.as_ref().unwrap()["address"], "bcrt1qexample");
        assert_eq!(order.remote.as_ref().unwrap()["returnCase"]["state"], "requested");

        apply_reply(&mut order, &json!({ "kind": "return", "action": "accept", "text": "Send it back." }), network);
        assert_eq!(order.status, crate::watch::SHIPPED);
        assert_eq!(order.remote.as_ref().unwrap()["returnCase"]["state"], "accepted");
        assert_eq!(order.remote.as_ref().unwrap()["returnCase"]["decision"], "Send it back.");
        assert_eq!(order.payment.as_ref().unwrap()["amountSats"], 25_800);
    }
}
