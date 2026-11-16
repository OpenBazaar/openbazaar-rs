//! A Lightning sale between two nodes.
//!
//! The buyer's node asks with the order option `rail` = `ln`. The seller's
//! node checks that its store can take Lightning now ([`Node::ln_sale_refusal`]),
//! prices the order in sats as any sale, and issues an invoice from its own
//! Lightning node. The terms carry `rail`, `invoice`, `paymentHash`,
//! `amountMsat`, `expiresAt` and `network`, and no bitcoin `address`.
//!
//! The buyer's node checks the invoice ([`payment_from_terms`]) and keeps it
//! as the order's `payment` with `issuer` `remote`, so paying it is the same
//! spend-ticket path as any other remote invoice, including the routing fee
//! cap. This node still refuses to pay an invoice it issued itself.

use openbazaar_lightning::ldk_node::bitcoin::Network;
use openbazaar_lightning::{decode_invoice, Incoming};
use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::network::NetworkMode;
use crate::watch::{AWAITING_PAYMENT, PAID};
use crate::{Node, OrderError};

pub(crate) const NOT_TAKEN: &str = "This store does not take Lightning. Pay in bitcoin.";
const OFF: &str = "This store's node has Lightning switched off right now. Pay in bitcoin, or ask the seller to switch it back on.";
const STARTING: &str = "This store's Lightning node is still starting. Try again in a minute, or pay in bitcoin.";
/// The store's record takes Lightning, but its node answered with bitcoin terms.
pub(crate) const NO_LN_TERMS: &str = "This store's node did not answer with a Lightning invoice. It may need an update. Order again and pay in bitcoin.";

impl Node {
    /// Why the own store declines a Lightning order right now, in the buyer's words.
    pub(crate) fn ln_sale_refusal(&self) -> Option<&'static str> {
        if self.ln_blocked() {
            return Some(super::MAINNET_BLOCKED);
        }
        if !self.own_record().and_then(|store| store.details).is_some_and(|details| details.accepts_lightning) {
            return Some(NOT_TAKEN);
        }
        if self.ln.config().is_none() {
            return Some(OFF);
        }
        if self.ln.rail().is_none() {
            return Some(STARTING);
        }
        None
    }

    /// The `payment` of a Lightning sale of `total` sats: an invoice from this node's Lightning node.
    pub(crate) fn ln_sale_payment(&self, total: u64, order_id: &str) -> Result<Value, String> {
        self.ln_payment(total, order_id).map_err(|err| match err {
            OrderError::Failed(detail) => detail,
            other => other.to_string(),
        })
    }

    /// Mark open Lightning sales paid when their invoice is claimed, and tell the buyer. Returns how many changed.
    pub(crate) fn ln_update_sales(&self) -> usize {
        let Some(rail) = self.ln.rail().cloned() else { return 0 };
        let open: Vec<(String, String)> = self
            .sales()
            .list()
            .filter(|sale| sale["payment"]["rail"] == "ln" && sale["status"] == AWAITING_PAYMENT)
            .filter_map(|sale| Some((sale["id"].as_str()?.to_string(), sale["payment"]["paymentHash"].as_str()?.to_string())))
            .collect();
        let mut changed = 0;
        for (id, hash) in open {
            let Incoming::Paid { received_msat, lsp_fee_msat, at } = rail.incoming(&hash) else { continue };
            let result = self.sales().update(&id, |sale| {
                sale["status"] = json!(PAID);
                sale["txids"] = json!([hash.clone()]);
                sale["payment"]["receivedMsat"] = json!(received_msat);
                sale["payment"]["lspFeeMsat"] = json!(lsp_fee_msat);
                sale["payment"]["paidAt"] = json!(at);
            });
            let sale = match result {
                Ok(Some(sale)) if sale["status"] == PAID => sale,
                Ok(_) => continue,
                Err(err) => {
                    eprintln!("ob-node: sale {id}: {err}");
                    continue;
                }
            };
            changed += 1;
            if let Err(err) = self.tell_buyer(&sale, &self.paid_reply(&sale)) {
                eprintln!("ob-node: sale {id}: {err}");
            }
        }
        changed
    }
}

/// Add the Lightning fields of a sale's `payment` to its terms.
pub(crate) fn add_terms(terms: &mut Value, payment: &Value) {
    for key in ["rail", "invoice", "paymentHash", "amountMsat", "expiresAt", "network", "jit"] {
        terms[key] = payment[key].clone();
    }
}

fn named_network(name: &str) -> Option<Network> {
    Some(match name {
        "signet" => Network::Signet,
        "testnet4" => Network::Testnet4,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        "bitcoin" => Network::Bitcoin,
        _ => return None,
    })
}

/// On the buyer's node: the order's `payment` from the seller's Lightning terms, or why they are refused.
pub(crate) fn payment_from_terms(body: &Value, mode: NetworkMode) -> Result<Value, String> {
    let invoice = body["invoice"].as_str().unwrap_or_default();
    let Some(network) = body["network"].as_str().and_then(named_network) else {
        return Err(NO_LN_TERMS.into());
    };
    if body["rail"] != "ln" || invoice.is_empty() {
        return Err(NO_LN_TERMS.into());
    }
    let allowed = match mode {
        NetworkMode::Mainnet => network == Network::Bitcoin,
        NetworkMode::Testnet4 => network != Network::Bitcoin,
    };
    if !allowed {
        return Err("The seller's Lightning invoice is for another network.".into());
    }
    let info = decode_invoice(invoice, network).map_err(|err| err.to_string())?;
    let amount_msat = info.amount_msat.ok_or_else(|| "The seller's Lightning invoice has no amount.".to_string())?;
    let amount_sats = body["amountSats"].as_u64().unwrap_or(0);
    if amount_msat != amount_sats.saturating_mul(1_000) || body["amountMsat"].as_u64() != Some(amount_msat) || amount_sats == 0 {
        return Err("The seller's Lightning amount does not match its invoice.".into());
    }
    if body["paymentHash"].as_str() != Some(info.payment_hash.as_str()) {
        return Err("The seller's Lightning invoice does not match its payment hash.".into());
    }
    if info.expires_at <= now_secs() {
        return Err("The seller's Lightning invoice has expired.".into());
    }
    Ok(json!({
        "rail": "ln",
        "network": body["network"],
        "invoice": invoice,
        "uri": format!("lightning:{}", invoice.to_uppercase()),
        "paymentHash": info.payment_hash,
        "amountMsat": amount_msat,
        "amountSats": amount_sats,
        "expiresAt": info.expires_at,
        "jit": body["jit"],
        "issuer": "remote",
        "watched": ["ln"],
    }))
}
