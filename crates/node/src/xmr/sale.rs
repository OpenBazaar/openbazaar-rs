//! A Monero sale between two nodes.
//!
//! The buyer's node asks with the order option `rail` = `xmr`. The seller's
//! node checks that its store can take a Monero order now
//! ([`Node::xmr_sale_refusal`]), prices the order in sats as any sale, fixes
//! the XMR amount at its own rate, and names a fresh subaddress of the own
//! store's wallet (the node wallet). Its terms carry `rail`, `subaddress`,
//! `amountPiconero`, `satsPerXmr`, `amountSats` and `network`, and no
//! bitcoin `address`.
//!
//! The buyer's node needs no daemon. It checks the subaddress and the
//! arithmetic ([`payment_from_terms`]), keeps the terms as the order's
//! `payment`, and from there follows the seller's word ([`follow_seller`]):
//! whenever the seller's wallet sees the sale change, its reply carries
//! `xmr` with `status`, `receivedPiconero`, `confirmations`, `txids`, and a
//! counter `seq`, since the outbox does not keep replies in order. That
//! reply is the usual "paid" when the sale becomes paid, and kind
//! "progress" before.

use openbazaar_xmr::amount::{format_xmr, piconero_for_sats};
use openbazaar_xmr::{address_for, monero_uri, XmrNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

use super::checkout::amount_of;
use crate::buyer_orders::BuyerOrder;
use crate::watch::{direct_progress, AWAITING_PAYMENT, PAID, PAYMENT_SEEN};
use crate::Node;

pub(crate) const NOT_TAKEN: &str = "This store does not take Monero. Pay in bitcoin.";
/// The store's record takes Monero, but its node answered with bitcoin terms: it runs a version from before Monero sales.
pub(crate) const NO_XMR_TERMS: &str = "This store's node did not answer with Monero terms. It may need an update. Order again and pay in bitcoin.";

/// What the buyer's node is told about a Monero sale, less the counter.
fn told(sale: &Value) -> Value {
    let payment = &sale["payment"];
    json!({
        "status": sale["status"],
        "receivedPiconero": payment["receivedPiconero"],
        "confirmations": sale["confirmations"],
        "txids": sale["txids"],
    })
}

/// Why the own store declines a Monero order right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NotReady {
    Off,
    NoWallet,
    NoPrice,
}

impl NotReady {
    pub(crate) fn buyer(self) -> &'static str {
        match self {
            NotReady::Off => "This store's node has Monero switched off right now. Pay in bitcoin, or ask the seller to switch it back on.",
            NotReady::NoWallet => "This store's node is still preparing its Monero wallet. Try again in a few minutes, or pay in bitcoin.",
            NotReady::NoPrice => "This store's node has no XMR price right now. Try again in a few minutes, or pay in bitcoin.",
        }
    }
}

impl Node {
    /// Why the own store declines a Monero order right now, in the buyer's words.
    pub(crate) fn xmr_sale_refusal(&self) -> Option<&'static str> {
        if !self
            .own_record()
            .and_then(|store| store.details)
            .is_some_and(|details| details.accepts_xmr)
        {
            return Some(NOT_TAKEN);
        }
        self.own_xmr_ready().err().map(NotReady::buyer)
    }

    fn own_xmr_ready(&self) -> Result<(), NotReady> {
        if self.xmr.chain().is_none() {
            return Err(NotReady::Off);
        }
        if self.xmr.buyer().wallet.is_none() {
            return Err(NotReady::NoWallet);
        }
        self.xmr
            .rate
            .sats_per_xmr()
            .map(|_| ())
            .ok_or(NotReady::NoPrice)
    }

    /// The `payment` of a Monero sale of `total_sats`, at a fresh subaddress of the own store's wallet.
    pub(crate) fn xmr_sale_payment(&self, total_sats: u64) -> Result<Value, String> {
        let sats_per_xmr = self
            .xmr
            .rate
            .sats_per_xmr()
            .ok_or("there is no XMR price")?;
        self.xmr_issue_payment(total_sats, sats_per_xmr, &mut self.xmr.buyer())
            .map_err(|err| err.to_string())
    }

    /// Update every open Monero sale from the own store's wallet and tell its buyer. Returns how many changed.
    pub(super) fn xmr_update_sales(&self) -> usize {
        let open: Vec<(String, u32)> = self
            .sales()
            .list()
            .filter(|sale| {
                sale["payment"]["rail"] == "xmr"
                    && (sale["status"] == AWAITING_PAYMENT || sale["status"] == PAYMENT_SEEN)
            })
            .filter_map(|sale| {
                Some((
                    sale["id"].as_str()?.to_string(),
                    u32::try_from(sale["payment"]["subaddressIndex"].as_u64()?).ok()?,
                ))
            })
            .collect();
        let mut changed = 0;
        for (id, index) in open {
            let Some(received) = self
                .xmr
                .buyer()
                .wallet
                .as_ref()
                .map(|wallet| wallet.received(index))
            else {
                continue;
            };
            let mut before = Value::Null;
            let result = self.sales().update(&id, |sale| {
                before = told(sale);
                let due = amount_of(&sale["payment"], "amountPiconero");
                sale["status"] = json!(direct_progress(due, received.total, received.confirmed));
                sale["payment"]["receivedPiconero"] = json!(received.total.to_string());
                sale["payment"]["remainingPiconero"] =
                    json!(due.saturating_sub(received.total).to_string());
                sale["payment"]["overpaidPiconero"] =
                    json!(received.total.saturating_sub(due).to_string());
                sale["confirmations"] = json!(received.confirmations.unwrap_or(0));
                sale["txids"] = json!(received.txids);
                if told(sale) != before {
                    sale["payment"]["seq"] =
                        json!(sale["payment"]["seq"].as_u64().unwrap_or(0) + 1);
                }
            });
            let sale = match result {
                Ok(Some(sale)) if told(&sale) != before => sale,
                Ok(_) => continue,
                Err(err) => {
                    eprintln!("ob-node: sale {id}: {err}");
                    continue;
                }
            };
            changed += 1;
            let mut reply = if sale["status"] == PAID {
                self.paid_reply(&sale)
            } else {
                json!({ "v": 1, "kind": "progress" })
            };
            reply["xmr"] = told(&sale);
            reply["xmr"]["seq"] = sale["payment"]["seq"].clone();
            if let Err(err) = self.tell_buyer(&sale, &reply) {
                eprintln!("ob-node: sale {id}: {err}");
            }
        }
        changed
    }
}

/// Add the Monero fields of a sale's `payment` to its terms.
pub(crate) fn add_terms(terms: &mut Value, payment: &Value) {
    for key in [
        "rail",
        "subaddress",
        "amountPiconero",
        "satsPerXmr",
        "network",
    ] {
        terms[key] = payment[key].clone();
    }
}

/// On the buyer's node: the order's `payment` from the seller's Monero terms, or why they are refused.
pub(crate) fn payment_from_terms(body: &Value, network: XmrNetwork) -> Result<Value, String> {
    let address = body["subaddress"].as_str().unwrap_or_default();
    if body["rail"] != "xmr" || address.is_empty() {
        return Err(NO_XMR_TERMS.into());
    }
    let parsed = address_for(address, network)
        .ok()
        .filter(|parsed| parsed.is_subaddress() && body["network"] == network.as_str());
    let Some(parsed) = parsed else {
        return Err("The seller's Monero address is not a subaddress on this network.".into());
    };
    let piconero = amount_of(body, "amountPiconero");
    let (sats, sats_per_xmr) = (
        body["amountSats"].as_u64().unwrap_or(0),
        body["satsPerXmr"].as_u64().unwrap_or(0),
    );
    if piconero_for_sats(sats, sats_per_xmr) != Some(piconero) || piconero == 0 {
        return Err("The seller's Monero amount does not match its price and rate.".into());
    }
    Ok(json!({
        "rail": "xmr",
        "network": network.as_str(),
        "subaddress": parsed.to_string(),
        "amountPiconero": piconero.to_string(),
        "amountXmr": format_xmr(piconero),
        "uri": monero_uri(&parsed, piconero),
        "satsPerXmr": sats_per_xmr,
        "priceSats": sats,
        "receivedPiconero": "0",
        "confirmationsNeeded": CONFIRMATIONS,
        // The seller's node watches the subaddress. This node has no view of it.
        "watched": [],
    }))
}

/// On the buyer's node: take the seller's word (`told`, the `xmr` of a reply) on an unpaid Monero order.
/// Only a "paid" reply marks the order paid.
pub(crate) fn follow_seller(order: &mut BuyerOrder, told: &Value) {
    let open = order.status == AWAITING_PAYMENT || order.status == PAYMENT_SEEN;
    let Some(payment) = order.payment.as_mut().filter(|payment| {
        open && payment["rail"] == "xmr" && told["seq"].as_u64() > payment["sellerSeq"].as_u64()
    }) else {
        return;
    };
    let (due, received) = (
        amount_of(payment, "amountPiconero"),
        amount_of(told, "receivedPiconero"),
    );
    payment["sellerSeq"] = told["seq"].clone();
    payment["receivedPiconero"] = json!(received.to_string());
    payment["remainingPiconero"] = json!(due.saturating_sub(received).to_string());
    payment["overpaidPiconero"] = json!(received.saturating_sub(due).to_string());
    order.confirmations = told["confirmations"]
        .as_u64()
        .unwrap_or(0)
        .min(u64::from(u32::MAX)) as u32;
    order.txids = told["txids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|txid| txid.as_str().map(str::to_string))
        .collect();
    if told["status"] == PAYMENT_SEEN || told["status"] == AWAITING_PAYMENT {
        order.status = told["status"].as_str().unwrap_or_default().to_string();
    }
}
