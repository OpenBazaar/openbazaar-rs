//! A Zcash sale between two nodes.
//!
//! The buyer's node asks with the order option `rail` = `zec`. The seller's
//! node checks that its store can take a Zcash order now
//! ([`Node::own_zec_ready`]), prices the order in sats as any sale, fixes
//! the ZEC amount at its own rate, and names a fresh address of the own
//! store's wallet. Its terms carry `rail`, `zcashAddress`, `amountZatoshi`,
//! `satsPerZec`, `amountSats`, `network`, `pool` and `expiresAt`, and no
//! bitcoin `address`.
//!
//! The buyer's node needs no indexer. It checks the address and the
//! arithmetic ([`payment_from_terms`]), keeps the terms as the order's
//! `payment`, and from there follows the seller's word ([`follow_seller`]):
//! whenever the seller's wallet sees the sale change, its reply carries
//! `zec` with `status`, `receivedZatoshi`, `confirmations`, `txids`,
//! `expired`, `seenInTime`, and a counter `seq`, since the outbox does not
//! keep replies in order. That reply is the usual "paid" when the sale
//! becomes paid, and kind "progress" before.

use openbazaar_zec::amount::{format_zec, zatoshi_for_sats};
use openbazaar_zec::{is_order_address, zcash_uri, ZecNetwork, CONFIRMATIONS};
use serde_json::{json, Value};

use super::checkout::{amount_of, record_received};
use super::own::NotReady;
use crate::buyer_orders::{now_secs, BuyerOrder};
use crate::watch::{AWAITING_PAYMENT, PAID, PAYMENT_SEEN};
use crate::Node;

pub(crate) const NOT_TAKEN: &str = "This store does not take Zcash. Pay in bitcoin.";
/// The store's record takes Zcash, but its node answered with bitcoin terms: it runs a version from before Zcash sales.
pub(crate) const NO_ZEC_TERMS: &str = "This store's node did not answer with Zcash terms. It may need an update. Order again and pay in bitcoin.";

/// What the buyer's node is told about a Zcash sale, less the counter.
fn told(sale: &Value) -> Value {
    let payment = &sale["payment"];
    json!({
        "status": sale["status"],
        "receivedZatoshi": payment["receivedZatoshi"],
        "confirmations": sale["confirmations"],
        "txids": sale["txids"],
        "expired": payment["expired"] == true,
        "seenInTime": payment["seenInTime"] == true,
    })
}

impl Node {
    /// Why the own store declines a Zcash order right now, in the buyer's words.
    pub(crate) fn zec_sale_refusal(&self) -> Option<&'static str> {
        if !self.own_record().and_then(|store| store.details).is_some_and(|details| details.accepts_zec) {
            return Some(NOT_TAKEN);
        }
        self.own_zec_ready().err().map(NotReady::buyer)
    }

    /// The `payment` of a Zcash sale of `total_sats`, at a fresh address of the own store's wallet.
    pub(crate) fn zec_sale_payment(&self, total_sats: u64) -> Result<Value, String> {
        let sats_per_zec = self.sats_per_zec().ok_or("there is no ZEC price")?;
        self.zec_quote(total_sats, sats_per_zec, &mut self.zec.own()).map_err(|err| err.to_string())
    }

    /// Update every open Zcash sale from the own store's wallet and tell its buyer. Returns how many changed.
    pub(super) fn zec_update_sales(&self) -> usize {
        let open: Vec<(String, u32)> = self
            .sales()
            .list()
            .filter(|sale| sale["payment"]["rail"] == "zec" && (sale["status"] == AWAITING_PAYMENT || sale["status"] == PAYMENT_SEEN))
            .filter_map(|sale| Some((sale["id"].as_str()?.to_string(), u32::try_from(sale["payment"]["addressIndex"].as_u64()?).ok()?)))
            .collect();
        let (now, mut changed) = (now_secs(), 0);
        for (id, index) in open {
            let Some(received) = self.zec.own().wallet.as_ref().map(|wallet| wallet.received(index)) else { continue };
            let mut before = Value::Null;
            let result = self.sales().update(&id, |sale| {
                before = told(sale);
                let was = sale["status"].as_str().unwrap_or_default().to_string();
                sale["status"] = json!(record_received(&mut sale["payment"], &was, &received, now));
                sale["confirmations"] = json!(received.confirmations.unwrap_or(0));
                sale["txids"] = json!(received.txids);
                if told(sale) != before {
                    sale["payment"]["seq"] = json!(sale["payment"]["seq"].as_u64().unwrap_or(0) + 1);
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
            let mut reply = if sale["status"] == PAID { self.paid_reply(&sale) } else { json!({ "v": 1, "kind": "progress" }) };
            reply["zec"] = told(&sale);
            reply["zec"]["seq"] = sale["payment"]["seq"].clone();
            if let Err(err) = self.tell_buyer(&sale, &reply) {
                eprintln!("ob-node: sale {id}: {err}");
            }
        }
        changed
    }
}

/// Add the Zcash fields of a sale's `payment` to its terms.
pub(crate) fn add_terms(terms: &mut Value, payment: &Value) {
    for key in ["rail", "zcashAddress", "amountZatoshi", "satsPerZec", "network", "pool", "expiresAt"] {
        terms[key] = payment[key].clone();
    }
}

/// On the buyer's node: the order's `payment` from the seller's Zcash terms, or why they are refused.
pub(crate) fn payment_from_terms(body: &Value, network: ZecNetwork) -> Result<Value, String> {
    let address = body["zcashAddress"].as_str().unwrap_or_default();
    if body["rail"] != "zec" || address.is_empty() {
        return Err(NO_ZEC_TERMS.into());
    }
    // Only an address with one Orchard-protocol receiver, on this network, is sure to be paid shielded into Ironwood.
    if body["network"] != network.as_str() || body["pool"] != "ironwood" || !is_order_address(address, network) {
        return Err("The seller's Zcash address is not a shielded Ironwood address on this network.".into());
    }
    let zatoshi = amount_of(body, "amountZatoshi");
    let (sats, sats_per_zec) = (body["amountSats"].as_u64().unwrap_or(0), body["satsPerZec"].as_u64().unwrap_or(0));
    let uri = zcash_uri(address, zatoshi).ok().filter(|_| zatoshi_for_sats(sats, sats_per_zec) == Some(zatoshi) && body["expiresAt"].is_u64());
    let Some(uri) = uri else {
        return Err("The seller's Zcash amount does not match its price and rate.".into());
    };
    Ok(json!({
        "rail": "zec",
        "network": network.as_str(),
        "pool": "ironwood",
        "zcashAddress": address,
        "amountZatoshi": zatoshi.to_string(),
        "amountZec": format_zec(zatoshi),
        "uri": uri,
        "satsPerZec": sats_per_zec,
        "priceSats": sats,
        "receivedZatoshi": "0",
        "expiresAt": body["expiresAt"],
        "confirmationsNeeded": CONFIRMATIONS,
        // The seller's node watches the address. This node has no view of it.
        "watched": [],
    }))
}

/// On the buyer's node: take the seller's word (`told`, the `zec` of a reply) on an unpaid Zcash order.
/// Only a "paid" reply marks the order paid.
pub(crate) fn follow_seller(order: &mut BuyerOrder, told: &Value) {
    let open = order.status == AWAITING_PAYMENT || order.status == PAYMENT_SEEN;
    let Some(payment) = order.payment.as_mut().filter(|payment| open && payment["rail"] == "zec" && told["seq"].as_u64() > payment["sellerSeq"].as_u64()) else {
        return;
    };
    let (due, received) = (amount_of(payment, "amountZatoshi"), amount_of(told, "receivedZatoshi"));
    payment["sellerSeq"] = told["seq"].clone();
    payment["receivedZatoshi"] = json!(received.to_string());
    payment["remainingZatoshi"] = json!(due.saturating_sub(received).to_string());
    payment["overpaidZatoshi"] = json!(received.saturating_sub(due).to_string());
    payment["expired"] = json!(told["expired"] == true);
    payment["seenInTime"] = json!(told["seenInTime"] == true);
    order.confirmations = told["confirmations"].as_u64().unwrap_or(0).min(u64::from(u32::MAX)) as u32;
    order.txids = told["txids"].as_array().into_iter().flatten().filter_map(|txid| txid.as_str().map(str::to_string)).collect();
    if told["status"] == PAYMENT_SEEN || told["status"] == AWAITING_PAYMENT {
        order.status = told["status"].as_str().unwrap_or_default().to_string();
    }
}
