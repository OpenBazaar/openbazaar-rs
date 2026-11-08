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

use openbazaar_protocol::{Listing, ListingKind};
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

fn refused(message: impl Into<String>) -> MailError {
    MailError::Refused(message.into())
}

fn retry(message: impl std::fmt::Display) -> MailError {
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
    pub(crate) fn receive_store_mail(&self, bytes: &[u8]) -> Result<(), MailError> {
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
        self.receive_buyer_message(opened)
    }

    fn receive_order(&self, opened: Opened<Order>) -> Result<(), MailError> {
        let order = opened.body;
        let id = hex(&order.reply.drop.0);
        let sender = hex(&opened.sender.0);
        if let Some(sale) = self.sales().get(&id) {
            return if sale["buyerSender"] == sender.as_str() {
                Ok(())
            } else {
                Err(refused("that order id is taken"))
            };
        }
        let reply_url = order.options.get("replyUrl").cloned().unwrap_or_default();
        let reply_url = crate::peer_http::sale_base(&reply_url).map_err(refused)?;
        let route = Route { url: reply_url, key: *order.reply.key.as_bytes(), drop: order.reply.drop.0 };
        if self.on_vacation() {
            self.queue_reply(&route, &json!({ "v": 1, "kind": "declined", "reason": crate::vacation::AWAY_MESSAGE })).map_err(retry)?;
            return Ok(());
        }
        let priced = match order.terms.lines.is_empty() {
            true => self.price_order(&order),
            false => self.price_terms_order(&id, &order, &opened.sender.0)?,
        };
        match priced {
            Ok(mut priced) => {
                let checked = priced.checked.take();
                let result = self.open_sale(id.clone(), sender, &order, route, priced);
                match (&result, checked) {
                    (Ok(()), Some(checked)) => self.record_terms(&id, &checked),
                    (Err(_), Some(_)) => self.release_units(&id),
                    _ => {}
                }
                result
            }
            Err(reason) => {
                self.queue_reply(&route, &json!({ "v": 1, "kind": "declined", "reason": reason })).map_err(retry)?;
                Ok(())
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
        Ok(Priced { lines: values, items, shipping, digital, physical, quote: Value::Null, checked: None })
    }

    fn open_sale(&self, id: String, sender: String, order: &Order, route: Route, priced: Priced) -> Result<(), MailError> {
        let total = priced.items.checked_add(priced.shipping).ok_or_else(|| refused("the total overflows"))?;
        let address = self.buyer_wallet().fresh_address().map_err(retry)?.to_string();
        let (ship_to, note) = match (priced.physical, priced.digital) {
            (true, true) => (order.ship_to.clone(), order.options.get("delivery").cloned().unwrap_or_default()),
            (true, false) => (order.ship_to.clone(), String::new()),
            _ => (String::new(), order.ship_to.clone()),
        };
        let sale = json!({
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
        self.sales().insert(sale).map_err(retry)?;
        self.queue_reply(&route, &terms).map_err(retry)
    }

    fn receive_buyer_message(&self, opened: Opened<Vec<u8>>) -> Result<(), MailError> {
        let body: Value = serde_json::from_slice(&opened.body).map_err(|_| refused("that message could not be read"))?;
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
        let reply = self.with_pending_receipts(&sale, reply);
        let route = Route::of(&sale).map_err(|err| StoreError::Failed(format!("{err:?}")))?;
        self.queue_reply(&route, &reply).map_err(StoreError::Failed)?;
        let updated = self
            .sales()
            .update(id, |sale| {
                sale["status"] = json!(SHIPPED);
                sale["fulfillment"] = fulfillment;
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

    /// Seal `body` to the buyer's reply key and queue it for the buyer's node.
    fn queue_reply(&self, route: &Route, body: &Value) -> Result<(), String> {
        let bytes = body_bytes(body);
        let key = ReplyPublic::from_bytes(route.key).map_err(|err| err.to_string())?;
        let from = self.sale_letter_key(&route.drop)?;
        let sealed = seal_reply(&bytes, &key, &from).map_err(|err| err.to_string())?;
        let frame = reply_frame(DropId(route.drop), &sealed);
        let url = format!("{}{REPLY_MAIL}", route.url);
        self.order_outbox.queue(mail_key(&url, &route.drop, &bytes), &url, &frame)
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
