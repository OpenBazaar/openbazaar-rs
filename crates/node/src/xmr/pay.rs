//! Paying a Monero order from the node's own Monero wallet.
//!
//! The same step-up as bitcoin ([`crate::spend_tickets`]): a quote fixes the
//! order, the amount in piconero, the destination subaddress, and a fee cap
//! (the quoted fee plus half); the passphrase turns it into a one-time
//! ticket; the ticket is redeemed once, for that order, at
//! `POST /xmr/orders/{id}/pay`, which answers 202. The Monero thread then
//! derives the spend key from the phrase (refused while locked), fetches
//! ring members, signs one payment plus change, and broadcasts it. It
//! refuses a transaction whose amount, destination, or fee differs from the
//! ticket. One order per transaction; sellers are never batched.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use openbazaar_xmr::amount::format_xmr;
use openbazaar_xmr::{address_for, quote_payment, sign_payment, MoneroAddress, SpendKey};
use serde_json::{json, Value};

use crate::buyer_orders::BuyerOrder;
use crate::network::NetworkMode;
use crate::pay_order::{PayError, TERMS_CHANGED};
use crate::spend_tickets::{OrderTerms, TicketError, FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_keys;
use crate::watch::AWAITING_PAYMENT;
use crate::Node;

use super::guard;

/// What one Monero spend is allowed to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XmrTerms {
    pub order_id: String,
    pub amount: u64,
    pub fee_cap: u64,
    pub destination: String,
}

impl OrderTerms for XmrTerms {
    fn order_id(&self) -> &str {
        &self.order_id
    }
}

impl XmrTerms {
    pub(crate) fn value(&self, network: &str) -> Value {
        json!({
            "orderId": self.order_id,
            "network": network,
            "amountPiconero": self.amount.to_string(),
            "amountXmr": format_xmr(self.amount),
            "feeCapPiconero": self.fee_cap.to_string(),
            "feeCapXmr": format_xmr(self.fee_cap),
            "destination": self.destination,
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

pub(crate) struct XmrJobs {
    queue: Sender<(String, XmrTerms)>,
    pending: Mutex<Receiver<(String, XmrTerms)>>,
}

impl XmrJobs {
    pub(crate) fn new() -> Self {
        let (queue, pending) = mpsc::channel();
        Self { queue, pending: Mutex::new(pending) }
    }
}

fn ticket_error(err: TicketError) -> PayError {
    match err {
        TicketError::Unknown => PayError::Conflict("That spend ticket is used, expired, or unknown. Confirm the payment again.".into()),
        TicketError::OtherOrder => PayError::Conflict("That spend ticket is for another order.".into()),
    }
}

fn send_error(err: openbazaar_xmr::SendError) -> PayError {
    match err {
        openbazaar_xmr::SendError::FeeAboveCap { .. } => PayError::Conflict(format!("{err} Nothing was sent.")),
        openbazaar_xmr::SendError::Library(_) | openbazaar_xmr::SendError::WrongKey => PayError::Failed(err.to_string()),
        _ => PayError::Invalid(err.to_string()),
    }
}

impl Node {
    fn xmr_chain_or_refuse(&self) -> Result<std::sync::Arc<dyn openbazaar_xmr::XmrChain>, PayError> {
        self.xmr.chain().cloned().ok_or_else(|| PayError::Invalid("Monero is off on this node.".into()))
    }

    /// Where a Monero order's payment goes and how much is still due.
    fn xmr_target(&self, order: &BuyerOrder) -> Result<(MoneroAddress, u64), PayError> {
        if order.settlement != "direct" {
            return Err(PayError::Invalid(super::checkout::ESCROW_IS_BITCOIN_ONLY.into()));
        }
        let payment = order.payment.as_ref().filter(|payment| payment["rail"] == "xmr");
        let payment = payment.ok_or_else(|| PayError::Invalid("This order is not paid in Monero.".into()))?;
        if let Some(txid) = &order.pay_txid {
            return Err(PayError::Conflict(format!("This node already paid this order in {txid}.")));
        }
        if order.paying {
            return Err(PayError::Conflict("A payment for this order is already being sent.".into()));
        }
        if order.status != AWAITING_PAYMENT {
            return Err(PayError::Conflict(format!("This order is {}.", order.status)));
        }
        let amount = |key: &str| payment[key].as_str().and_then(|text| text.parse::<u64>().ok()).unwrap_or(0);
        let due = amount("amountPiconero").saturating_sub(amount("receivedPiconero"));
        if due == 0 {
            return Err(PayError::Conflict("Nothing is left to pay on this order.".into()));
        }
        let text = payment["subaddress"].as_str().unwrap_or_default();
        Ok((address_for(text, self.xmr.xmr_network()).map_err(PayError::Invalid)?, due))
    }

    /// `POST /xmr/orders/{id}/pay/quote`.
    pub(crate) fn xmr_quote(&self, id: &str) -> Result<(String, XmrTerms), PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        let chain = self.xmr_chain_or_refuse()?;
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        let (destination, due) = self.xmr_target(&order)?;
        let fee_rate = chain.fee_rate().map_err(PayError::Chain)?;
        let wallet = self.xmr.buyer().wallet.clone().ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let quote = quote_payment(&wallet, &destination, due, fee_rate).map_err(send_error)?;
        let terms = XmrTerms {
            order_id: order.id.clone(),
            amount: due,
            fee_cap: (u128::from(quote.fee) * u128::from(FEE_CAP_PERCENT)).div_ceil(100) as u64,
            destination: destination.to_string(),
        };
        Ok((self.lock.xmr_tickets.quote(terms.clone(), self.lock.now()), terms))
    }

    /// `POST /xmr/orders/{id}/pay`: check, mark `paying`, and queue.
    pub(crate) fn xmr_queue_payment(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let ticket = self.xmr_pay_request(body)?;
        self.lock.spend_key()?;
        self.xmr_chain_or_refuse()?;
        let terms = self.lock.xmr_tickets.redeem(&ticket, id, self.lock.now()).map_err(ticket_error)?;
        let mut orders = self.orders();
        let order = orders.get(id).cloned().ok_or(PayError::NotFound)?;
        let (destination, due) = self.xmr_target(&order)?;
        if terms.destination != destination.to_string() || terms.amount != due {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        orders
            .update(id, |order| {
                order.paying = true;
                order.pay_error = None;
            })
            .map_err(PayError::Failed)?;
        self.xmr.jobs.queue.send((id.to_string(), terms)).map_err(|err| PayError::Failed(err.to_string()))?;
        self.lock.touch();
        orders.get_value(id).ok_or(PayError::NotFound)
    }

    fn xmr_pay_request(&self, body: &Value) -> Result<String, PayError> {
        let map = body.as_object().ok_or_else(|| PayError::Invalid("The request could not be read.".into()))?;
        if map.keys().any(|key| key != "confirm" && key != "ticket") {
            return Err(PayError::Invalid("That field is not part of a payment.".into()));
        }
        if self.network == NetworkMode::Mainnet && map.get("confirm") != Some(&Value::Bool(true)) {
            return Err(PayError::Invalid("This node is on mainnet and pays with real Monero. Send \"confirm\": true to pay.".into()));
        }
        match map.get("ticket") {
            Some(Value::String(ticket)) if !ticket.is_empty() => Ok(ticket.clone()),
            _ => Err(PayError::Invalid("Every payment needs a spend ticket. Quote it, confirm it, then send the ticket.".into())),
        }
    }

    /// Run one queued Monero payment, waiting up to `wait` for one. Returns whether one ran.
    pub(crate) fn run_xmr_pay_job(&self, wait: Duration) -> bool {
        let next = guard(&self.xmr.jobs.pending).recv_timeout(wait);
        let Ok((id, terms)) = next else { return false };
        let result = self.xmr_pay_now(&id, &terms);
        let error = match &result {
            Ok(txid) => {
                eprintln!("ob-node: paid order {id} in Monero tx {txid}");
                None
            }
            Err(err) => {
                eprintln!("ob-node: Monero pay {id}: {err:?}");
                Some(match err {
                    PayError::Failed(_) => "The Monero payment failed. Details are in the node log.".to_string(),
                    PayError::Locked(_) => crate::lock_http::LOCKED_MESSAGE.to_string(),
                    PayError::NotFound => "That order is not on this node.".to_string(),
                    PayError::Invalid(message) | PayError::Conflict(message) | PayError::Chain(message) => message.clone(),
                })
            }
        };
        let saved = self.orders().update(&id, |order| {
            order.paying = false;
            order.pay_error = error;
            if let Ok(txid) = &result {
                order.pay_txid = Some(txid.clone());
            }
        });
        if let Err(err) = saved {
            eprintln!("ob-node: order {id}: {err}");
        }
        true
    }

    /// Run every queued Monero payment now (tests drive the worker this way).
    #[cfg(test)]
    pub(crate) fn run_xmr_pay_jobs(&self) -> usize {
        let mut ran = 0;
        while self.run_xmr_pay_job(Duration::ZERO) {
            ran += 1;
        }
        ran
    }

    fn xmr_pay_now(&self, id: &str, terms: &XmrTerms) -> Result<String, PayError> {
        let chain = self.xmr_chain_or_refuse()?;
        let mut order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        order.paying = false;
        let (destination, due) = self.xmr_target(&order)?;
        if terms.order_id != order.id || terms.destination != destination.to_string() || terms.amount != due {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        let spend = {
            let key = self.lock.spend_key()?;
            let dir = self.buyer_wallet().dir.clone();
            let seed = wallet_keys::spend_seed(&dir, &key, self.network).map_err(|err| PayError::Failed(err.to_string()))?;
            SpendKey::from_seed_secret(&seed.monero_secret())
        };
        let wallet = self.xmr.buyer().wallet.clone().ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let fee_rate = chain.fee_rate().map_err(PayError::Chain)?;
        let quote = quote_payment(&wallet, &destination, due, fee_rate).map_err(send_error)?;
        let tip = chain.tip().map_err(PayError::Chain)?;
        let decoyed = chain.decoys(quote.inputs.clone(), tip).map_err(PayError::Chain)?;
        let signed = sign_payment(&wallet, &quote, decoyed, &spend, terms.fee_cap).map_err(send_error)?;
        drop(spend);
        chain.publish(&signed.tx).map_err(PayError::Chain)?;
        let txid = crate::vault_key::encode_hex(&signed.txid);
        let mut slot = self.xmr.buyer();
        if let Some(wallet) = slot.wallet.as_mut() {
            wallet.mark_spent(&signed.inputs, signed.txid);
        }
        if let Err(err) = slot.save(&self.xmr.vault) {
            eprintln!("ob-node: Monero wallet state after {txid}: {err}");
        }
        Ok(txid)
    }
}
