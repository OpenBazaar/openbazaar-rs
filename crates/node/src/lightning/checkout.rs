//! The Lightning rail at checkout, and Lightning order status.
//!
//! A direct-pay package may carry `"rail": "ln"`. The seller's Lightning
//! node issues a BOLT 11 invoice for this order only, for the order total in
//! sats; when it has no channel with room, the invoice routes through its LSP
//! (a just-in-time channel). The order's `payment` then has no `address`, so
//! the bitcoin watcher and pay worker never touch it.
//!
//! Status: Awaiting payment -> Paid, the moment the seller's node claims the
//! payment (Lightning has no confirmations). An invoice expires after an
//! hour; the order stays Awaiting payment with `payment.expired`.

use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::watch::{AWAITING_PAYMENT, PAID};
use crate::{Node, OrderError};

pub(crate) const ESCROW_STAYS_ON_CHAIN: &str = "Escrow stays on-chain. Pay over Lightning with direct pay.";
pub(crate) const INVOICE_SECS: u32 = 3_600;

/// `true` for the Lightning rail. Refuses Lightning with escrow.
pub(crate) fn ln_rail_of(value: Option<&Value>, escrow: bool) -> Result<bool, OrderError> {
    match value.and_then(Value::as_str) {
        Some("ln") if escrow => Err(OrderError::Invalid(ESCROW_STAYS_ON_CHAIN)),
        Some("ln") => Ok(true),
        _ => Ok(false),
    }
}

/// `acceptsLightning` on each listing of a catalog value, and the node's top-level `lightning` state.
pub(crate) fn add_ln_flags(node: &Node, value: &mut Value) {
    let running = node.ln.rail().is_some();
    if let Some(listings) = value["listings"].as_array_mut() {
        for listing in listings {
            let seller = listing["seller"].as_str().unwrap_or_default();
            listing["acceptsLightning"] = json!(running && node.accepts_lightning(seller));
        }
    }
    value["lightning"] = json!({
        "enabled": node.ln.config().is_some(),
        "running": running,
        "network": node.ln.config().map(|config| super::options::network_name(config.network)),
    });
}

impl Node {
    /// Everything a Lightning package needs, checked before any letter is sealed.
    pub(crate) fn ln_ready(&self, seller: &str) -> Result<(), OrderError> {
        if self.ln.config().is_none() {
            return Err(OrderError::Invalid("Lightning is off on this node. Start it with --lightning to pay over Lightning."));
        }
        if !self.accepts_lightning(seller) {
            return Err(OrderError::Invalid("This store does not accept Lightning."));
        }
        if self.ln.rail().is_none() {
            return Err(OrderError::Invalid("The Lightning node is still starting. Try again in a minute."));
        }
        if !self.ln_sellers().iter().any(|name| name == seller) {
            return Err(OrderError::Invalid("This store's Lightning invoices cannot be fetched yet."));
        }
        Ok(())
    }

    /// The seller's invoice for one order of `total` sats.
    pub(crate) fn ln_payment(&self, total: u64, order_id: &str) -> Result<Value, OrderError> {
        let rail = self.ln.rail().ok_or(OrderError::Invalid("The Lightning node is still starting. Try again in a minute."))?;
        let amount_msat = total.checked_mul(1_000).ok_or(OrderError::Invalid("That total is too large."))?;
        let short: String = order_id.chars().take(12).collect();
        let invoice = rail.receive(amount_msat, &format!("OpenBazaar order {short}"), INVOICE_SECS).map_err(|err| {
            eprintln!("ob-node: Lightning invoice: {err}");
            OrderError::Failed(format!("The seller's Lightning node could not issue an invoice: {err}"))
        })?;
        Ok(json!({
            "rail": "ln",
            "invoice": invoice.bolt11,
            "uri": format!("lightning:{}", invoice.bolt11.to_uppercase()),
            "paymentHash": invoice.payment_hash,
            "amountMsat": invoice.amount_msat,
            "amountSats": total,
            "expiresAt": invoice.expires_at,
            "jit": invoice.jit,
            "lspFeeCapMsat": invoice.lsp_fee_cap_msat,
            "issuer": "local",
            "watched": ["ln"],
        }))
    }

    /// Move Lightning orders forward from the issuer's view (orders whose
    /// invoice this node issued) and the payer's view (orders this node paid).
    /// Returns how many changed.
    pub(crate) fn ln_update_orders(&self) -> usize {
        let Some(rail) = self.ln.rail().cloned() else { return 0 };
        let open: Vec<(String, Value)> = self
            .orders()
            .on_network()
            .filter(|order| order.status != PAID && order.settlement == "direct")
            .filter_map(|order| {
                let payment = order.payment.clone()?;
                (payment["rail"] == "ln").then(|| (order.id.clone(), payment))
            })
            .collect();
        let mut changed = 0;
        for (id, payment) in open {
            let hash = payment["paymentHash"].as_str().unwrap_or_default().to_string();
            let incoming = if payment["issuer"] == "local" { rail.incoming(&hash) } else { openbazaar_lightning::Incoming::Unknown };
            let outgoing = rail.outgoing(&hash);
            let expired = payment["expiresAt"].as_u64().is_some_and(|at| at <= now_secs());
            let result = self.orders().update(&id, |order| {
                let Some(payment) = order.payment.as_mut() else { return };
                let before = (order.status.clone(), order.paying, order.pay_error.clone(), payment.clone());
                match (&incoming, &outgoing) {
                    (openbazaar_lightning::Incoming::Paid { received_msat, lsp_fee_msat, at }, _) => {
                        order.status = PAID.to_string();
                        order.paying = false;
                        payment["receivedMsat"] = json!(received_msat);
                        payment["lspFeeMsat"] = json!(lsp_fee_msat);
                        payment["paidAt"] = json!(at);
                    }
                    (_, openbazaar_lightning::Outgoing::Paid { fee_msat, at }) => {
                        order.status = PAID.to_string();
                        order.paying = false;
                        payment["routingFeeMsat"] = json!(fee_msat);
                        payment["paidAt"] = json!(at);
                    }
                    (_, openbazaar_lightning::Outgoing::Failed) if order.paying => {
                        order.paying = false;
                        order.pay_error = Some("The Lightning payment found no route. Try again, or pay the invoice from another wallet.".into());
                    }
                    _ => {
                        order.status = AWAITING_PAYMENT.to_string();
                        payment["expired"] = json!(expired);
                    }
                }
                if before != (order.status.clone(), order.paying, order.pay_error.clone(), payment.clone()) {
                    changed += 1;
                }
            });
            if let Err(err) = result {
                eprintln!("ob-node: order {id}: {err}");
            }
        }
        changed
    }
}
