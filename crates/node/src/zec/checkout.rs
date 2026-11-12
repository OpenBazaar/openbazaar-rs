//! The Zcash rail at checkout, and Zcash order status.
//!
//! A direct-pay package may carry `"rail": "zec"`. The seller's Zcash wallet
//! on this node hands out an address for this order only, whose one receiver
//! is Orchard-protocol, so the payment lands in Ironwood. The price in sats
//! becomes zatoshi at the current rate, rounded up. The order's `payment`
//! names that address `zcashAddress` and has no `address` field, so the
//! bitcoin watcher and pay worker never touch it.
//!
//! Status, from the seller wallet's view of that address, Ironwood notes
//! only: Awaiting payment (nothing, or less than due: `receivedZatoshi`
//! shows what arrived) -> Payment seen (all of it, under 30 confirmations)
//! -> Paid (30 confirmations). More than due is Paid with `overpaidZatoshi`.
//! The node has no view of the mempool, so a payment is seen from its block.

use openbazaar_zec::amount::{format_zec, zatoshi_for_sats};
use openbazaar_zec::{zcash_uri, CONFIRMATIONS};
use serde_json::{json, Value};

use crate::watch::{direct_progress, PAID};
use crate::{Node, OrderError};

pub(crate) const ESCROW_IS_BITCOIN_ONLY: &str = "Escrow is Bitcoin-only. Pay in Zcash with direct pay.";
pub(crate) const NOT_SYNCED: &str = "This node has not read the Zcash chain yet, so a Zcash payment could be missed. Try again in a few minutes, or pay in bitcoin.";

/// `true` for the Zcash rail. Refuses Zcash with escrow.
pub(crate) fn rail_of(value: Option<&Value>, escrow: bool) -> Result<bool, OrderError> {
    match value.and_then(Value::as_str) {
        Some("zec") if escrow => Err(OrderError::Invalid(ESCROW_IS_BITCOIN_ONLY)),
        Some("zec") => Ok(true),
        _ => Ok(false),
    }
}

/// `acceptsZec` on each listing of a catalog value, and the node's top-level `zec` state.
pub(crate) fn add_zec_flags(node: &Node, value: &mut Value) {
    let enabled = node.zec.chain().is_some();
    if let Some(listings) = value["listings"].as_array_mut() {
        for listing in listings {
            let seller = listing["seller"].as_str().unwrap_or_default();
            listing["acceptsZec"] = json!(enabled && node.accepts_zec(seller));
        }
    }
    value["zec"] = json!({ "enabled": enabled, "network": node.zec.zec_network().as_str(), "pool": "ironwood" });
}

fn amount_of(payment: &Value, key: &str) -> u64 {
    payment[key].as_str().and_then(|text| text.parse().ok()).unwrap_or(0)
}

impl Node {
    /// Sats per ZEC now, from Kraken's ZEC/USD and this node's BTC/USD.
    pub(crate) fn sats_per_zec(&self) -> Option<u64> {
        let btc_usd = self.rates.btc_price("USD").filter(|_| self.rates.answered_within(super::rate::USABLE));
        self.zec.rate.sats_per_zec(btc_usd)
    }

    /// Everything a Zcash package needs, checked before any letter is sealed.
    pub(crate) fn zec_ready(&self, seller: &str) -> Result<(), OrderError> {
        if self.zec.chain().is_none() {
            return Err(OrderError::Invalid("Zcash is off on this node, so a Zcash order cannot be watched."));
        }
        if !self.accepts_zec(seller) {
            return Err(OrderError::Invalid("This store does not accept Zcash."));
        }
        match self.zec.store(seller) {
            // The first sync starts a little before the tip, so it would skip a payment to an address handed out earlier.
            Some(slot) if slot.wallet.as_ref().is_some_and(|wallet| wallet.scanned().is_none()) => {
                return Err(OrderError::Invalid(NOT_SYNCED));
            }
            Some(slot) if slot.wallet.is_some() => {}
            Some(_) => return Err(OrderError::Locked(crate::wallet_lock::State::Locked.as_str())),
            None => return Err(OrderError::Invalid("That seller has no wallet on this node.")),
        }
        if self.sats_per_zec().is_none() {
            return Err(OrderError::Invalid("There is no ZEC price right now, so a Zcash order cannot be priced. Try again, or pay in bitcoin."));
        }
        Ok(())
    }

    /// The `payment` of a Zcash order: a fresh address of the seller and the amount in zatoshi.
    pub(crate) fn zec_payment(&self, total_sats: u64, seller: &str) -> Result<Value, OrderError> {
        let sats_per_zec = self.sats_per_zec().ok_or(OrderError::Invalid("There is no ZEC price right now."))?;
        let zatoshi = zatoshi_for_sats(total_sats, sats_per_zec).ok_or(OrderError::Invalid("That order is too large to price in Zcash."))?;
        let mut slot = self.zec.store(seller).ok_or(OrderError::Invalid("That seller has no wallet on this node."))?;
        let wallet = slot.wallet.as_mut().ok_or(OrderError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let (index, address) = wallet.issue_address();
        slot.save(&self.zec.vault).map_err(|err| OrderError::Failed(err.to_string()))?;
        let uri = zcash_uri(&address, zatoshi).map_err(OrderError::Failed)?;
        Ok(json!({
            "rail": "zec",
            "network": self.zec.zec_network().as_str(),
            "pool": "ironwood",
            "zcashAddress": address,
            "addressIndex": index,
            "amountZatoshi": zatoshi.to_string(),
            "amountZec": format_zec(zatoshi),
            "uri": uri,
            "satsPerZec": sats_per_zec,
            "priceSats": total_sats,
            "receivedZatoshi": "0",
            "confirmationsNeeded": CONFIRMATIONS,
            "watched": ["zec"],
        }))
    }

    /// Update every open Zcash order from its seller's wallet. Returns how many changed.
    pub(crate) fn zec_update_orders(&self) -> usize {
        let open: Vec<(String, String, Value)> = self
            .orders()
            .on_network()
            .filter(|order| order.status != PAID && order.settlement == "direct")
            .filter_map(|order| {
                let payment = order.payment.clone()?;
                (payment["rail"] == "zec").then(|| (order.id.clone(), order.seller.clone(), payment))
            })
            .collect();
        let mut changed = 0;
        for (id, seller, payment) in open {
            let Some(index) = payment["addressIndex"].as_u64().and_then(|n| u32::try_from(n).ok()) else { continue };
            let received = match self.zec.store(&seller) {
                Some(slot) => match &slot.wallet {
                    Some(wallet) => wallet.received(index),
                    None => continue,
                },
                None => continue,
            };
            let due = amount_of(&payment, "amountZatoshi");
            let status = direct_progress(due, received.total, received.confirmed);
            let confirmations = received.confirmations.unwrap_or(0).min(u64::from(u32::MAX)) as u32;
            let result = self.orders().update(&id, |order| {
                let Some(payment) = order.payment.as_mut() else { return };
                if order.status == status
                    && amount_of(payment, "receivedZatoshi") == received.total
                    && order.confirmations == confirmations
                    && order.txids == received.txids
                {
                    return;
                }
                order.status = status.to_string();
                order.confirmations = confirmations;
                order.txids = received.txids.clone();
                payment["receivedZatoshi"] = json!(received.total.to_string());
                payment["remainingZatoshi"] = json!(due.saturating_sub(received.total).to_string());
                payment["overpaidZatoshi"] = json!(received.total.saturating_sub(due).to_string());
                changed += 1;
            });
            if let Err(err) = result {
                eprintln!("ob-node: order {id}: {err}");
            }
        }
        changed
    }
}
