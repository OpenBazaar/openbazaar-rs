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
//! order, a "shipped" reply carries the tracking or the digital goods, and
//! `POST /orders/{id}/complete` tells the seller it arrived.

use openbazaar_protocol::Listing;
use serde_json::{json, Value};
use tormail::{open_reply, seal_message, ReplyDelivery, SealedLetter};

use crate::escrow_mail::hex;
use crate::escrow_outbox::Sent;
use crate::order_mail::{body_bytes, mail_key, MailError, STORE_MAIL};
use crate::orders::package_shipping;
use crate::watch::{AWAITING_PAYMENT, AWAITING_SELLER, COMPLETED, DECLINED, PAYMENT_SEEN, SHIPPED};
use crate::{Node, OrderError};

fn refused(message: impl Into<String>) -> MailError {
    MailError::Refused(message.into())
}

fn retry(message: impl std::fmt::Display) -> MailError {
    MailError::Retry(message.to_string())
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
    /// Refuses what such a store cannot take, and the own store: escrow, and
    /// a rail only a seller wallet on this node can take (`local_rail`:
    /// Monero or Zcash), since a remote seller's terms carry bitcoin only.
    pub(crate) fn remote_package(&self, listing: &Listing, direct: bool, local_rail: bool) -> Result<Option<String>, OrderError> {
        if listing.store_id == self.own_store {
            return Err(OrderError::Invalid("That is your own store."));
        }
        let followed = self.replicas().url_of(&listing.store_id).map(str::to_string);
        let Some(url) = followed.or_else(|| self.directory_sale_url(&listing.store_id)) else {
            return Ok(None);
        };
        if !direct || local_rail {
            return Err(OrderError::Invalid("This store takes direct bitcoin payment only."));
        }
        if self.sale_reply_url().is_none() {
            return Err(OrderError::Invalid("This node has no address for the seller's reply."));
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
        Ok(json!({
            "url": url,
            "drop": hex(&drop.0),
            "estimateSats": estimate,
            "fiat": fiat,
            "delivered": false,
        }))
    }

    /// Queue the sealed order for the seller's node and try it once.
    pub(crate) fn deliver_remote_order(&self, id: &str, url: &str, checkout_id: &str, listing: &Listing, letter: &SealedLetter) {
        let target = format!("{url}{STORE_MAIL}");
        let conversation = [checkout_id.as_bytes(), listing.store_id.as_bytes()].concat();
        let bytes = letter.to_bytes();
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
fn apply_reply(order: &mut crate::buyer_orders::BuyerOrder, body: &Value, network: crate::NetworkMode) {
    let remote = order.remote.clone().unwrap_or_default();
    match body["kind"].as_str() {
        Some("terms") if order.status == AWAITING_SELLER => {
            let amount = body["amountSats"].as_u64().unwrap_or(0);
            let estimate = remote["estimateSats"].as_u64().unwrap_or(0);
            let address = body["address"].as_str().unwrap_or("");
            let limit = ceiling(estimate, remote["fiat"].as_bool().unwrap_or(false));
            let problem = crate::checkout_terms::quote_problem(&remote, body);
            let accepted = match crate::network::address_for(address, network) {
                Err(_) => Err("The seller's payment address is not valid here.".to_string()),
                Ok(_) if problem.is_some() => Err(problem.unwrap_or_default()),
                Ok(_) if amount == 0 || amount > limit => {
                    Err(format!("The seller asked {amount} sats; checkout showed {estimate}. Nothing was paid."))
                }
                Ok(address) => Ok(address),
            };
            match accepted {
                Ok(address) => {
                    order.items_sats = body["itemsSats"].as_u64().unwrap_or(order.items_sats);
                    order.shipping_sats = body["shippingSats"].as_u64().unwrap_or(order.shipping_sats);
                    order.total_sats = amount;
                    order.payment = Some(json!({
                        "network": network.as_str(),
                        "onchain": address.to_string(),
                        "address": address.to_string(),
                        "amountSats": amount,
                        "bip21": crate::network::bip21(&address, bitcoin::Amount::from_sat(amount)),
                        "watched": ["onchain"],
                    }));
                    order.status = AWAITING_PAYMENT.to_string();
                    if let Some(remote) = order.remote.as_mut().filter(|_| !body["quote"].is_null()) {
                        remote["sellerQuote"] = body["quote"].clone();
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
        Some("completed") => {
            if let Some(remote) = order.remote.as_mut() {
                remote["sellerCompleted"] = json!(true);
            }
        }
        _ => {}
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
}
