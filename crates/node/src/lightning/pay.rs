//! Paying a Lightning order from this node, and the channel work that moves
//! money: all behind the passphrase.
//!
//! A payment uses the same step-up as bitcoin and Monero: a quote fixes the
//! order, the invoice's payment hash, the amount, and a routing fee cap (1 %
//! of the amount, at least 10 sats); the passphrase turns it into a one-time
//! ticket; `POST /ln/orders/{id}/pay` redeems it once for that order. The
//! payment starts at once and the order shows `paying` until it settles.
//!
//! Opening and closing a channel and sweeping LDK's on-chain funds take the
//! passphrase in the request itself, as a fee bump does. A sweep goes to a
//! fresh address of the node's own bitcoin wallet; the client cannot name one.

use openbazaar_lightning::{decode_invoice, PeerAddress};
use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::network::NetworkMode;
use crate::pay_order::{PayError, TERMS_CHANGED};
use crate::spend_tickets::{OrderTerms, TicketError, TICKET_TTL};
use crate::wallet_lock::State;
use crate::watch::AWAITING_PAYMENT;
use crate::Node;

/// What one Lightning payment is allowed to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LnTerms {
    pub order_id: String,
    pub payment_hash: String,
    pub amount_msat: u64,
    pub fee_cap_msat: u64,
}

impl OrderTerms for LnTerms {
    fn order_id(&self) -> &str {
        &self.order_id
    }
}

impl LnTerms {
    pub(crate) fn value(&self, network: &str) -> Value {
        json!({
            "orderId": self.order_id,
            "network": network,
            "paymentHash": self.payment_hash,
            "amountMsat": self.amount_msat,
            "amountSats": self.amount_msat.div_ceil(1_000),
            "feeCapMsat": self.fee_cap_msat,
            "feeCapSats": self.fee_cap_msat.div_ceil(1_000),
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

fn ticket_error(err: TicketError) -> PayError {
    match err {
        TicketError::Unknown => PayError::Conflict("That spend ticket is used, expired, or unknown. Confirm the payment again.".into()),
        TicketError::OtherOrder => PayError::Conflict("That spend ticket is for another order.".into()),
    }
}

fn unlocked(node: &Node) -> Result<(), PayError> {
    match node.lock.state() {
        State::Unlocked => Ok(()),
        state => Err(PayError::Locked(state.as_str())),
    }
}

/// The routing fee cap for `amount_msat`: 1 %, at least 10 sats.
pub(crate) fn fee_cap(amount_msat: u64) -> u64 {
    (amount_msat / 100).max(10_000)
}

impl Node {
    pub(crate) fn ln_network_name(&self) -> &'static str {
        self.ln.config().map_or("off", |config| super::options::network_name(config.network))
    }

    fn ln_rail_or_refuse(&self) -> Result<std::sync::Arc<dyn openbazaar_lightning::LightningRail>, PayError> {
        if self.ln.config().is_none() && self.ln.rail().is_none() {
            return Err(PayError::Invalid("Lightning is off on this node. Start it with --lightning.".into()));
        }
        self.ln.rail().cloned().ok_or_else(|| PayError::Chain("The Lightning node is still starting. Try again in a minute.".into()))
    }

    /// The terms a payment of order `id` would have now.
    fn ln_terms(&self, id: &str) -> Result<(LnTerms, String), PayError> {
        let rail = self.ln_rail_or_refuse()?;
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        let payment = order.payment.clone().unwrap_or(Value::Null);
        if payment["rail"] != "ln" {
            return Err(PayError::Invalid("That order is not paid over Lightning.".into()));
        }
        if order.status != AWAITING_PAYMENT || order.paying {
            return Err(PayError::Conflict("That order is not waiting for a payment.".into()));
        }
        let invoice = payment["invoice"].as_str().unwrap_or_default().to_string();
        let network = self.ln.config().map(|config| config.network).unwrap_or(rail.info().network);
        let info = decode_invoice(&invoice, network).map_err(|err| PayError::Invalid(err.to_string()))?;
        if info.expires_at <= now_secs() {
            return Err(PayError::Invalid("That invoice has expired.".into()));
        }
        if info.payee == rail.info().node_id {
            return Err(PayError::Invalid("This node issued that invoice. Pay it from another Lightning wallet.".into()));
        }
        let amount_msat = info.amount_msat.ok_or_else(|| PayError::Invalid("That invoice has no amount.".into()))?;
        if payment["paymentHash"].as_str() != Some(info.payment_hash.as_str()) || payment["amountMsat"].as_u64() != Some(amount_msat) {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        let terms = LnTerms { order_id: order.id, payment_hash: info.payment_hash, amount_msat, fee_cap_msat: fee_cap(amount_msat) };
        Ok((terms, invoice))
    }

    /// `POST /ln/orders/{id}/pay/quote`. Needs an unlocked wallet.
    pub(crate) fn ln_quote(&self, id: &str) -> Result<(String, LnTerms), PayError> {
        unlocked(self)?;
        let (terms, _) = self.ln_terms(id)?;
        let rail = self.ln_rail_or_refuse()?;
        let spendable = rail.balances().lightning_sats.saturating_mul(1_000);
        if spendable < terms.amount_msat.saturating_add(terms.fee_cap_msat) {
            return Err(PayError::Invalid(format!(
                "This node's Lightning balance is {} sats; this order needs {} sats plus up to {} sats in routing fees. Pay the invoice from another wallet.",
                spendable / 1_000,
                terms.amount_msat.div_ceil(1_000),
                terms.fee_cap_msat.div_ceil(1_000),
            )));
        }
        self.lock.touch();
        Ok((self.lock.ln_tickets.quote(terms.clone(), self.lock.now()), terms))
    }

    /// `POST /ln/orders/{id}/pay`: redeem a ticket and start the payment.
    pub(crate) fn ln_pay(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let map = body.as_object().ok_or_else(|| PayError::Invalid("The request could not be read.".into()))?;
        if map.keys().any(|key| key != "confirm" && key != "ticket") {
            return Err(PayError::Invalid("That field is not part of a payment.".into()));
        }
        if self.network == NetworkMode::Mainnet && map.get("confirm") != Some(&Value::Bool(true)) {
            return Err(PayError::Invalid("This node is on mainnet and pays with real bitcoin. Send \"confirm\": true to pay.".into()));
        }
        let ticket = match map.get("ticket") {
            Some(Value::String(ticket)) if !ticket.is_empty() => ticket.clone(),
            _ => return Err(PayError::Invalid("Every payment needs a spend ticket. Quote it, confirm it, then send the ticket.".into())),
        };
        unlocked(self)?;
        let allowed = self.lock.ln_tickets.redeem(&ticket, id, self.lock.now()).map_err(ticket_error)?;
        let (terms, invoice) = self.ln_terms(id)?;
        if terms != allowed {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        let rail = self.ln_rail_or_refuse()?;
        self.orders()
            .update(id, |order| {
                order.paying = true;
                order.pay_error = None;
            })
            .map_err(PayError::Failed)?;
        if let Err(err) = rail.pay(&invoice, allowed.fee_cap_msat) {
            eprintln!("ob-node: Lightning pay {id}: {err}");
            let message = err.to_string();
            let _ = self.orders().update(id, |order| {
                order.paying = false;
                order.pay_error = Some(message.clone());
            });
            return Err(PayError::Chain(message));
        }
        self.lock.touch();
        self.ln_update_orders();
        self.orders().get_value(id).ok_or(PayError::NotFound)
    }

    /// `POST /ln/spend-tickets`.
    pub(crate) fn ln_ticket(&self, quote: &str, passphrase: &str) -> Result<Value, crate::wallet_lock::LockError> {
        self.lock.verify(passphrase)?;
        match self.lock.ln_tickets.issue(quote, self.lock.now()) {
            Ok((ticket, terms)) => {
                let mut value = terms.value(self.ln_network_name());
                value["ticket"] = json!(ticket);
                Ok(value)
            }
            Err(_) => Err(crate::wallet_lock::LockError::Invalid("That quote is used or expired. Ask for a new one.".into())),
        }
    }

    /// The passphrase, again, for channel work.
    fn ln_step_up(&self, passphrase: &str) -> Result<(), PayError> {
        unlocked(self)?;
        self.lock.verify(passphrase).map_err(|err| match err {
            crate::wallet_lock::LockError::Locked(state) => PayError::Locked(state.as_str()),
            crate::wallet_lock::LockError::Wrong { .. } => PayError::Invalid("That passphrase is not right.".into()),
            crate::wallet_lock::LockError::Backoff { retry_after } => {
                PayError::Conflict(format!("Too many wrong passphrases. Try again in {retry_after} seconds."))
            }
            crate::wallet_lock::LockError::Invalid(message) | crate::wallet_lock::LockError::Failed(message) => PayError::Invalid(message),
        })?;
        self.lock.touch();
        Ok(())
    }

    pub(crate) fn ln_open_channel(&self, peer: &str, sats: u64, passphrase: &str) -> Result<Value, PayError> {
        let peer: PeerAddress = peer.parse().map_err(PayError::Invalid)?;
        if sats < 20_000 {
            return Err(PayError::Invalid("A channel needs at least 20000 sats.".into()));
        }
        self.ln_step_up(passphrase)?;
        let id = self.ln_rail_or_refuse()?.open_channel(&peer, sats).map_err(|err| PayError::Chain(err.to_string()))?;
        Ok(json!({ "channelId": id }))
    }

    pub(crate) fn ln_close_channel(&self, id: &str, force: bool, passphrase: &str) -> Result<Value, PayError> {
        self.ln_step_up(passphrase)?;
        self.ln_rail_or_refuse()?.close_channel(id, force).map_err(|err| PayError::Chain(err.to_string()))?;
        Ok(json!({ "channelId": id, "closing": true, "force": force }))
    }

    pub(crate) fn ln_sweep(&self, passphrase: &str) -> Result<Value, PayError> {
        self.ln_step_up(passphrase)?;
        let rail = self.ln_rail_or_refuse()?;
        if self.ln.config().is_some_and(|config| config.network != self.network.bitcoin()) {
            return Err(PayError::Invalid("Lightning runs on another network than this node's wallet; there is no wallet address to sweep to.".into()));
        }
        let address = self.buyer_wallet().fresh_address().map_err(|err| PayError::Failed(err.to_string()))?.to_string();
        let txid = rail.sweep_onchain(&address).map_err(|err| PayError::Chain(err.to_string()))?;
        Ok(json!({ "txid": txid, "address": address }))
    }
}
