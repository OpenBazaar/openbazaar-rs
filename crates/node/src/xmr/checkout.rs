//! The Monero rail at checkout, and Monero order status.
//!
//! A direct-pay package may carry `"rail": "xmr"` (the default is `"btc"`).
//! The seller's own Monero wallet hands out a subaddress used for this order
//! only, and the price in sats becomes piconero at the current XMR/BTC rate,
//! rounded up. The order's `payment` then has no `address` field, so the
//! bitcoin watcher and pay worker never touch it.
//!
//! Status, from the seller wallet's view of that subaddress:
//! Awaiting payment (nothing, or less than due: `receivedPiconero` shows
//! what arrived) -> Payment seen (all of it, under 10 confirmations) -> Paid
//! (10 confirmations). More than due is Paid with `overpaidPiconero`. The
//! node has no view of the mempool, so a payment is seen from its first
//! confirmation.

use openbazaar_xmr::amount::{format_xmr, piconero_for_sats};
use openbazaar_xmr::{monero_uri, CONFIRMATIONS};
use serde_json::{json, Value};

use crate::http::{json_body, json_error};
use crate::watch::{AWAITING_PAYMENT, PAID, PAYMENT_SEEN};
use crate::{catalog_value, Node, OrderError};

pub(crate) const ESCROW_IS_BITCOIN_ONLY: &str = "Escrow is Bitcoin-only. Pay in Monero with direct pay.";

/// `true` for the Monero rail. Refuses Monero with escrow.
pub(crate) fn rail_of(value: Option<&Value>, escrow: bool) -> Result<bool, OrderError> {
    match value.map(Value::as_str) {
        None | Some(Some("btc" | "ln")) => Ok(false),
        Some(Some("xmr")) if escrow => Err(OrderError::Invalid(ESCROW_IS_BITCOIN_ONLY)),
        Some(Some("xmr")) => Ok(true),
        Some(_) => Err(OrderError::Invalid("Choose the btc, xmr, or ln rail.")),
    }
}

/// `GET /catalog`, with `acceptsXmr` on each listing.
pub(crate) fn catalog_response(node: &Node, origin: Option<&str>) -> Vec<u8> {
    let mut value = match catalog_value(&node.catalog()) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("ob-node: {err}");
            return json_error("500 Internal Server Error", "The catalog could not be read.", origin);
        }
    };
    add_xmr_flags(node, &mut value);
    crate::lightning::add_ln_flags(node, &mut value);
    json_body("200 OK", &value, origin)
}

/// `acceptsXmr` on each listing of a catalog value, and the node's top-level `xmr` state.
pub(crate) fn add_xmr_flags(node: &Node, value: &mut Value) {
    let enabled = node.xmr.chain().is_some();
    if let Some(listings) = value["listings"].as_array_mut() {
        for listing in listings {
            let seller = listing["seller"].as_str().unwrap_or_default();
            listing["acceptsXmr"] = json!(enabled && node.accepts_xmr(seller));
        }
    }
    value["xmr"] = json!({ "enabled": enabled, "network": node.xmr.xmr_network().as_str() });
}

fn amount_of(payment: &Value, key: &str) -> u64 {
    payment[key].as_str().and_then(|text| text.parse().ok()).unwrap_or(0)
}

impl Node {
    /// Everything a Monero package needs, checked before any letter is sealed.
    pub(crate) fn xmr_ready(&self, seller: &str) -> Result<(), OrderError> {
        if self.xmr.chain().is_none() {
            return Err(OrderError::Invalid("Monero is off on this node. Start it with --xmr-daemon to pay in Monero."));
        }
        if !self.accepts_xmr(seller) {
            return Err(OrderError::Invalid("This store does not accept Monero."));
        }
        match self.xmr.store(seller) {
            Some(slot) if slot.wallet.is_some() => {}
            Some(_) => return Err(OrderError::Locked(crate::wallet_lock::State::Locked.as_str())),
            None => return Err(OrderError::Invalid("That seller has no wallet on this node.")),
        }
        if self.xmr.rate.sats_per_xmr().is_none() {
            return Err(OrderError::Invalid("There is no XMR price right now, so a Monero order cannot be priced. Try again, or pay in bitcoin."));
        }
        Ok(())
    }

    /// The `payment` of a Monero order: a fresh subaddress of the seller and the amount in piconero.
    pub(crate) fn xmr_payment(&self, total_sats: u64, seller: &str) -> Result<Value, OrderError> {
        let sats_per_xmr = self.xmr.rate.sats_per_xmr().ok_or(OrderError::Invalid("There is no XMR price right now."))?;
        let piconero = piconero_for_sats(total_sats, sats_per_xmr).ok_or(OrderError::Invalid("That order is too large to price in Monero."))?;
        let mut slot = self.xmr.store(seller).ok_or(OrderError::Invalid("That seller has no wallet on this node."))?;
        let wallet = slot.wallet.as_mut().ok_or(OrderError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let (index, address) = wallet.issue_subaddress();
        slot.save(&self.xmr.vault).map_err(|err| OrderError::Failed(err.to_string()))?;
        Ok(json!({
            "rail": "xmr",
            "network": self.xmr.xmr_network().as_str(),
            "subaddress": address.to_string(),
            "subaddressIndex": index,
            "amountPiconero": piconero.to_string(),
            "amountXmr": format_xmr(piconero),
            "uri": monero_uri(&address, piconero),
            "satsPerXmr": sats_per_xmr,
            "priceSats": total_sats,
            "receivedPiconero": "0",
            "confirmationsNeeded": CONFIRMATIONS,
            "watched": ["xmr"],
        }))
    }

    /// Update every open Monero order from its seller's wallet. Returns how many changed.
    pub(crate) fn xmr_update_orders(&self) -> usize {
        let open: Vec<(String, String, Value)> = self
            .orders()
            .on_network()
            .filter(|order| order.status != PAID && order.settlement == "direct")
            .filter_map(|order| {
                let payment = order.payment.clone()?;
                (payment["rail"] == "xmr").then(|| (order.id.clone(), order.seller.clone(), payment))
            })
            .collect();
        let mut changed = 0;
        for (id, seller, payment) in open {
            let Some(index) = payment["subaddressIndex"].as_u64().and_then(|n| u32::try_from(n).ok()) else { continue };
            let received = match self.xmr.store(&seller) {
                Some(slot) => match &slot.wallet {
                    Some(wallet) => wallet.received(index),
                    None => continue,
                },
                None => continue,
            };
            let due = amount_of(&payment, "amountPiconero");
            let status = if due > 0 && received.confirmed >= due {
                PAID
            } else if due > 0 && received.total >= due {
                PAYMENT_SEEN
            } else {
                AWAITING_PAYMENT
            };
            let confirmations = received.confirmations.unwrap_or(0).min(u64::from(u32::MAX)) as u32;
            let result = self.orders().update(&id, |order| {
                let Some(payment) = order.payment.as_mut() else { return };
                if order.status == status
                    && amount_of(payment, "receivedPiconero") == received.total
                    && order.confirmations == confirmations
                    && order.txids == received.txids
                {
                    return;
                }
                order.status = status.to_string();
                order.confirmations = confirmations;
                order.txids = received.txids.clone();
                payment["receivedPiconero"] = json!(received.total.to_string());
                payment["remainingPiconero"] = json!(due.saturating_sub(received.total).to_string());
                payment["overpaidPiconero"] = json!(received.total.saturating_sub(due).to_string());
                changed += 1;
            });
            if let Err(err) = result {
                eprintln!("ob-node: order {id}: {err}");
            }
        }
        changed
    }
}
