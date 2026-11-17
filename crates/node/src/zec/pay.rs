//! Paying a Zcash order from the node's own Zcash wallet.
//!
//! The same step-up as Monero ([`crate::spend_tickets`]): a quote fixes the
//! order, the amount in zatoshi, the destination, and a fee cap; the
//! passphrase turns it into a one-time ticket; the ticket is redeemed once
//! at `POST /zec/orders/{id}/pay`, which answers 202. The Zcash thread then
//! derives the spend key from the phrase (refused while locked), proves one
//! Ironwood payment plus change, and broadcasts it. ZIP 317 sets the fee,
//! so it does not move between the quote and the proof. Orchard notes are
//! not spent: moving them is a pool migration, not a payment.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use openbazaar_zec::amount::format_zec;
use openbazaar_zec::{quote_payment, sign_payment, SpendKey};
use serde_json::{json, Value};

use crate::buyer_orders::BuyerOrder;
use crate::network::NetworkMode;
use crate::pay_order::{PayError, TERMS_CHANGED};
use crate::spend_tickets::{OrderTerms, TicketError, FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_keys;
use crate::watch::AWAITING_PAYMENT;
use crate::Node;

use super::guard;

/// What one Zcash spend is allowed to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ZecTerms {
    pub order_id: String,
    pub amount: u64,
    pub fee_cap: u64,
    pub destination: String,
}

impl OrderTerms for ZecTerms {
    fn order_id(&self) -> &str {
        &self.order_id
    }
}

impl ZecTerms {
    pub(crate) fn value(&self, network: &str) -> Value {
        json!({
            "orderId": self.order_id,
            "network": network,
            "amountZatoshi": self.amount.to_string(),
            "amountZec": format_zec(self.amount),
            "feeCapZatoshi": self.fee_cap.to_string(),
            "feeCapZec": format_zec(self.fee_cap),
            "destination": self.destination,
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

pub(crate) struct ZecJobs {
    queue: Sender<(String, ZecTerms)>,
    pending: Mutex<Receiver<(String, ZecTerms)>>,
}

impl ZecJobs {
    pub(crate) fn new() -> Self {
        let (queue, pending) = mpsc::channel();
        Self {
            queue,
            pending: Mutex::new(pending),
        }
    }
}

fn ticket_error(err: TicketError) -> PayError {
    match err {
        TicketError::Unknown => PayError::Conflict(
            "That spend ticket is used, expired, or unknown. Confirm the payment again.".into(),
        ),
        TicketError::OtherOrder => {
            PayError::Conflict("That spend ticket is for another order.".into())
        }
    }
}

fn send_error(err: openbazaar_zec::SendError) -> PayError {
    match err {
        openbazaar_zec::SendError::FeeAboveCap { .. } => {
            PayError::Conflict(format!("{err} Nothing was sent."))
        }
        openbazaar_zec::SendError::Library(_) | openbazaar_zec::SendError::WrongKey => {
            PayError::Failed(err.to_string())
        }
        _ => PayError::Invalid(err.to_string()),
    }
}

fn shown(txid: &[u8; 32]) -> String {
    let mut reversed = *txid;
    reversed.reverse();
    crate::vault_key::encode_hex(&reversed)
}

impl Node {
    fn zec_chain_or_refuse(
        &self,
    ) -> Result<std::sync::Arc<dyn openbazaar_zec::ZecChain>, PayError> {
        self.zec
            .chain()
            .cloned()
            .ok_or_else(|| PayError::Invalid("Zcash is off on this node.".into()))
    }

    /// Where a Zcash order's payment goes and how much is still due.
    fn zec_target(&self, order: &BuyerOrder) -> Result<(String, u64), PayError> {
        if order.settlement != "direct" {
            return Err(PayError::Invalid(
                super::checkout::ESCROW_IS_BITCOIN_ONLY.into(),
            ));
        }
        let payment = order
            .payment
            .as_ref()
            .filter(|payment| payment["rail"] == "zec");
        let payment =
            payment.ok_or_else(|| PayError::Invalid("This order is not paid in Zcash.".into()))?;
        if let Some(txid) = &order.pay_txid {
            return Err(PayError::Conflict(format!(
                "This node already paid this order in {txid}."
            )));
        }
        if order.paying {
            return Err(PayError::Conflict(
                "A payment for this order is already being sent.".into(),
            ));
        }
        if order.status != AWAITING_PAYMENT {
            return Err(PayError::Conflict(format!(
                "This order is {}.",
                order.status
            )));
        }
        let amount = |key: &str| {
            payment[key]
                .as_str()
                .and_then(|text| text.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let due = amount("amountZatoshi").saturating_sub(amount("receivedZatoshi"));
        if due == 0 {
            return Err(PayError::Conflict(
                "Nothing is left to pay on this order.".into(),
            ));
        }
        let address = payment["zcashAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if address.is_empty() {
            return Err(PayError::Invalid("This order has no Zcash address.".into()));
        }
        Ok((address, due))
    }

    /// `POST /zec/orders/{id}/pay/quote`.
    pub(crate) fn zec_pay_quote(&self, id: &str) -> Result<(String, ZecTerms), PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        self.zec_chain_or_refuse()?;
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        let (destination, due) = self.zec_target(&order)?;
        let wallet = self
            .zec
            .own()
            .wallet
            .clone()
            .ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let quote = quote_payment(&wallet, &destination, due).map_err(send_error)?;
        let terms = ZecTerms {
            order_id: order.id.clone(),
            amount: due,
            fee_cap: (u128::from(quote.fee) * u128::from(FEE_CAP_PERCENT)).div_ceil(100) as u64,
            destination: quote.destination,
        };
        Ok((
            self.lock.zec_tickets.quote(terms.clone(), self.lock.now()),
            terms,
        ))
    }

    /// `POST /zec/orders/{id}/pay`: check, mark `paying`, and queue.
    pub(crate) fn zec_queue_payment(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        let ticket = self.zec_pay_request(body)?;
        self.lock.spend_key()?;
        self.zec_chain_or_refuse()?;
        let terms = self
            .lock
            .zec_tickets
            .redeem(&ticket, id, self.lock.now())
            .map_err(ticket_error)?;
        let mut orders = self.orders();
        let order = orders.get(id).cloned().ok_or(PayError::NotFound)?;
        let (destination, due) = self.zec_target(&order)?;
        if terms.destination != destination || terms.amount != due {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        orders
            .update(id, |order| {
                order.paying = true;
                order.pay_error = None;
            })
            .map_err(PayError::Failed)?;
        self.zec
            .jobs
            .queue
            .send((id.to_string(), terms))
            .map_err(|err| PayError::Failed(err.to_string()))?;
        self.lock.touch();
        orders.get_value(id).ok_or(PayError::NotFound)
    }

    fn zec_pay_request(&self, body: &Value) -> Result<String, PayError> {
        let map = body
            .as_object()
            .ok_or_else(|| PayError::Invalid("The request could not be read.".into()))?;
        if map.keys().any(|key| key != "confirm" && key != "ticket") {
            return Err(PayError::Invalid(
                "That field is not part of a payment.".into(),
            ));
        }
        if self.network == NetworkMode::Mainnet && map.get("confirm") != Some(&Value::Bool(true)) {
            return Err(PayError::Invalid(
                "This node is on mainnet and pays with real Zcash. Send \"confirm\": true to pay."
                    .into(),
            ));
        }
        match map.get("ticket") {
            Some(Value::String(ticket)) if !ticket.is_empty() => Ok(ticket.clone()),
            _ => Err(PayError::Invalid(
                "Every payment needs a spend ticket. Quote it, confirm it, then send the ticket."
                    .into(),
            )),
        }
    }

    /// Run one queued Zcash payment, waiting up to `wait` for one. Returns whether one ran.
    pub(crate) fn run_zec_pay_job(&self, wait: Duration) -> bool {
        let next = guard(&self.zec.jobs.pending).recv_timeout(wait);
        let Ok((id, terms)) = next else { return false };
        let result = self.zec_pay_now(&id, &terms);
        let error = match &result {
            Ok(txid) => {
                eprintln!("ob-node: paid order {id} in Zcash tx {txid}");
                None
            }
            Err(err) => {
                eprintln!("ob-node: Zcash pay {id}: {err:?}");
                Some(match err {
                    PayError::Failed(_) => {
                        "The Zcash payment failed. Details are in the node log.".to_string()
                    }
                    PayError::Locked(_) => crate::lock_http::LOCKED_MESSAGE.to_string(),
                    PayError::NotFound => "That order is not on this node.".to_string(),
                    PayError::Invalid(message)
                    | PayError::Conflict(message)
                    | PayError::Chain(message) => message.clone(),
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

    /// Run every queued Zcash payment now (tests drive the worker this way).
    #[cfg(test)]
    pub(crate) fn run_zec_pay_jobs(&self) -> usize {
        let mut ran = 0;
        while self.run_zec_pay_job(Duration::ZERO) {
            ran += 1;
        }
        ran
    }

    fn zec_pay_now(&self, id: &str, terms: &ZecTerms) -> Result<String, PayError> {
        let chain = self.zec_chain_or_refuse()?;
        let mut order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        order.paying = false;
        let (destination, due) = self.zec_target(&order)?;
        if terms.order_id != order.id || terms.destination != destination || terms.amount != due {
            return Err(PayError::Conflict(TERMS_CHANGED.into()));
        }
        let wallet = self
            .zec
            .own()
            .wallet
            .clone()
            .ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let spend = {
            let key = self.lock.spend_key()?;
            let dir = self.buyer_wallet().dir.clone();
            let seed = wallet_keys::spend_seed(&dir, &key, self.network)
                .map_err(|err| PayError::Failed(err.to_string()))?;
            SpendKey::from_seed(seed.zip32_seed()).map_err(send_error)?
        };
        let signed =
            sign_payment(&wallet, &spend, &destination, due, terms.fee_cap).map_err(send_error)?;
        drop(spend);
        chain.submit(&signed.raw).map_err(PayError::Chain)?;
        let txid = shown(&signed.txid);
        let mut slot = self.zec.own();
        if let Some(wallet) = slot.wallet.as_mut() {
            wallet.reserve(&signed.nullifiers, signed.txid, signed.expiry_height);
        }
        if let Err(err) = slot.save(&self.zec.vault) {
            eprintln!("ob-node: Zcash wallet state after {txid}: {err}");
        }
        Ok(txid)
    }
}
