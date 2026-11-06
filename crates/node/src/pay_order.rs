//! `POST /orders/{id}/pay`: pay one seller order from the node wallet.
//!
//! The route checks the request, marks the order `paying`, and answers 202
//! with the order. The pay worker thread then syncs, signs, and broadcasts,
//! and writes `payTxid` or `payError` into the order; `GET /orders/{id}`
//! shows the result and the watcher reports the status from the chain. A
//! slow chain server never holds up the HTTP accept loop. There is one
//! worker, so payments run one at a time and an order is never paid twice.
//!
//! One order, one recipient, one transaction. The only other output is the
//! wallet's own change, so two sellers are never paid in one transaction.
//! On mainnet the request must carry `"confirm": true`.
//!
//! The transaction is a normal one. Payjoin v2 is not attempted: the order's
//! payment terms carry no `pj=` endpoint, because the vendor side does not run
//! a Payjoin receiver session yet.
//!
//! Escrow funding is a testnet4-only demo (mainnet refuses it). It keeps the
//! in-process ceremony and enforces its order:
//! sign the funding transaction, run [`fund`] for that exact outpoint, check
//! and seal the presigned refund into the order record, and only then
//! broadcast. If the broadcast fails, a retry sends the same transaction so
//! the stored refund still spends it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::secp256k1::{PublicKey, Secp256k1};
use bitcoin::{Address, Amount, FeeRate, OutPoint, Transaction};
use openbazaar_escrow::{fund, FundParams};
use serde_json::{json, Value};

use crate::buyer_orders::{now_secs, BuyerOrder};
use crate::instructions::{ESCROW_MAINNET_REFUSAL, ESCROW_SPEND_FEE_SATS, ESCROW_TIMEOUT_BLOCKS};
use crate::network::{address_for, NetworkMode};
use crate::watch::{AWAITING_FUNDING, AWAITING_PAYMENT};
use crate::Node;

/// Lowest fee rate the node will use, in sat/vB.
pub const MIN_FEE_RATE: f64 = 1.0;
/// Highest fee rate the node will accept from the chain server, in sat/vB.
pub const MAX_FEE_RATE: f64 = 500.0;

#[derive(Debug)]
pub enum PayError {
    NotFound,
    Invalid(String),
    Conflict(String),
    Chain(String),
    Failed(String),
}

impl PayError {
    fn message(&self) -> String {
        match self {
            PayError::NotFound => "That order is not on this node.".into(),
            PayError::Invalid(message)
            | PayError::Conflict(message)
            | PayError::Chain(message)
            | PayError::Failed(message) => message.clone(),
        }
    }
}

/// Order ids waiting for the pay worker.
pub(crate) struct PayJobs {
    queue: Sender<String>,
    pending: Mutex<Receiver<String>>,
}

impl PayJobs {
    pub(crate) fn new() -> Self {
        let (queue, pending) = mpsc::channel();
        Self { queue, pending: Mutex::new(pending) }
    }
}

impl Node {
    /// Check the request and pay the order now, on this thread.
    pub fn pay_order(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        self.check_pay_request(body)?;
        self.pay_now(id)
    }

    /// Check the request, mark the order `paying`, and queue it for the pay
    /// worker. Returns the order as it is now.
    pub fn queue_payment(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        self.check_pay_request(body)?;
        let mut orders = self.orders();
        let order = orders.get(id).ok_or(PayError::NotFound)?;
        if order.paying {
            return Err(PayError::Conflict("A payment for this order is already being sent.".into()));
        }
        self.ready_to_pay(order)?;
        orders
            .update(id, |order| {
                order.paying = true;
                order.pay_error = None;
            })
            .map_err(PayError::Failed)?;
        self.pay_jobs
            .queue
            .send(id.to_string())
            .map_err(|err| PayError::Failed(err.to_string()))?;
        orders.get_value(id).ok_or(PayError::NotFound)
    }

    /// Run every queued payment now. Returns how many ran.
    pub fn run_pay_jobs(&self) -> usize {
        let mut ran = 0;
        loop {
            let next = self.pay_jobs.pending.lock().unwrap_or_else(|err| err.into_inner()).try_recv();
            let Ok(id) = next else {
                return ran;
            };
            self.run_pay_job(&id);
            ran += 1;
        }
    }

    /// Run queued payments as they arrive until `stop` is set.
    pub(crate) fn run_pay_worker(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            let next = self
                .pay_jobs
                .pending
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .recv_timeout(Duration::from_millis(250));
            match next {
                Ok(id) => self.run_pay_job(&id),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn run_pay_job(&self, id: &str) {
        let result = self.pay_now(id);
        let error = result.as_ref().err().map(PayError::message);
        match &result {
            Ok(paid) => eprintln!("ob-node: paid order {id} in {}", paid["txid"]),
            Err(err) => eprintln!("ob-node: pay {id}: {}", err.message()),
        }
        let saved = self.orders().update(id, |order| {
            order.paying = false;
            order.pay_error = error;
        });
        if let Err(err) = saved {
            eprintln!("ob-node: order {id}: {err}");
        }
    }

    fn pay_now(&self, id: &str) -> Result<Value, PayError> {
        let order = self.orders().get(id).cloned().ok_or(PayError::NotFound)?;
        self.ready_to_pay(&order)?;
        if order.settlement == "escrow" {
            self.fund_escrow(order)
        } else {
            self.pay_direct(order)
        }
    }

    /// Refuse an order this node must not pay right now.
    fn ready_to_pay(&self, order: &BuyerOrder) -> Result<(), PayError> {
        let escrow = order.settlement == "escrow";
        if escrow && self.network != NetworkMode::Testnet4 {
            return Err(PayError::Invalid(ESCROW_MAINNET_REFUSAL.into()));
        }
        if let Some(txid) = &order.pay_txid {
            let what = if escrow { "funded this escrow" } else { "paid this order" };
            return Err(PayError::Conflict(format!("This node already {what} in {txid}.")));
        }
        let waiting = if escrow { AWAITING_FUNDING } else { AWAITING_PAYMENT };
        if order.status != waiting {
            let what = if escrow { "escrow" } else { "order" };
            return Err(PayError::Conflict(format!("This {what} is {}.", order.status)));
        }
        if !escrow {
            if order.total_sats.saturating_sub(order.received_sats) == 0 {
                return Err(PayError::Conflict("Nothing is left to pay on this order.".into()));
            }
            self.order_address(order.payment.as_ref(), "address")?;
        }
        Ok(())
    }

    fn check_pay_request(&self, body: &Value) -> Result<(), PayError> {
        let Some(map) = body.as_object() else {
            return Err(PayError::Invalid("The request could not be read.".into()));
        };
        if map.keys().any(|key| key != "confirm") {
            return Err(PayError::Invalid("That field is not part of a payment.".into()));
        }
        let confirm = match map.get("confirm") {
            None => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err(PayError::Invalid("confirm must be true or false.".into())),
        };
        if self.network == NetworkMode::Mainnet && !confirm {
            return Err(PayError::Invalid(
                "This node is on mainnet and pays with real bitcoin. Send \"confirm\": true to pay.".into(),
            ));
        }
        Ok(())
    }

    fn pay_direct(&self, order: BuyerOrder) -> Result<Value, PayError> {
        let address = self.order_address(order.payment.as_ref(), "address")?;
        let due = order.total_sats.saturating_sub(order.received_sats);
        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let tx = self.sign_payment(&address, due, fee_rate)?;
        self.chain.broadcast(&tx).map_err(PayError::Chain)?;
        let txid = tx.compute_txid().to_string();
        self.after_broadcast(&order.id, tx.clone(), &txid)?;
        Ok(self.paid_value(&order.id, &tx, due, fee_rate))
    }

    fn fund_escrow(&self, order: BuyerOrder) -> Result<Value, PayError> {
        let escrow = order
            .escrow
            .clone()
            .ok_or_else(|| PayError::Invalid("This order has no escrow terms.".into()))?;
        if let Some(stored) = escrow.get("fundingTx").and_then(Value::as_str) {
            let tx: Transaction = deserialize_hex(stored).map_err(|err| PayError::Failed(err.to_string()))?;
            return self.broadcast_funding(&order, tx, escrow["fundingSats"].as_u64().unwrap_or(0), None);
        }
        let address = self.order_address(Some(&escrow), "address")?;
        let funding = escrow
            .get("fundingSats")
            .and_then(Value::as_u64)
            .ok_or_else(|| PayError::Invalid("This escrow has no funding amount.".into()))?;
        let buyer_payout = self.order_address(Some(&escrow), "buyerPayout")?;
        let vendor_payout = self.order_address(Some(&escrow), "vendorPayout")?;
        let index = |key: &str| {
            escrow
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| PayError::Invalid("This escrow has no key index.".into()))
        };
        let (buyer_index, vendor_index) = (index("buyerKeyIndex")?, index("vendorKeyIndex")?);

        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let tip = self.chain.tip_height().map_err(PayError::Chain)?;
        let tx = self.sign_payment(&address, funding, fee_rate)?;
        let vout = tx
            .output
            .iter()
            .position(|out| out.script_pubkey == address.script_pubkey())
            .ok_or_else(|| PayError::Failed("the funding output is missing".into()))?;
        let outpoint = OutPoint { txid: tx.compute_txid(), vout: vout as u32 };

        let buyer_secret = self.buyer_wallet().seed.escrow_key(buyer_index).secret_bytes();
        let vendor_secret = self
            .store_wallet(&order.seller)
            .map_err(|err| PayError::Failed(err.to_string()))?
            .seed
            .escrow_key(vendor_index)
            .secret_bytes();
        let timeout_height = tip
            .checked_add(ESCROW_TIMEOUT_BLOCKS)
            .ok_or_else(|| PayError::Chain("the chain tip is out of range".into()))?;
        let session = fund(FundParams {
            buyer_secret,
            vendor_secret,
            moderator_public: PublicKey::from_secret_key(&Secp256k1::new(), &self.keys.moderator).serialize(),
            funding_outpoint: outpoint,
            funding_value: Amount::from_sat(funding),
            fee: Amount::from_sat(ESCROW_SPEND_FEE_SATS),
            timeout_height,
            buyer_payout: buyer_payout.script_pubkey(),
            vendor_payout: vendor_payout.script_pubkey(),
        })
        .map_err(|err| PayError::Failed(err.to_string()))?;
        if session.funding_output().script_pubkey != address.script_pubkey() {
            return Err(PayError::Failed("the escrow keys do not match this address".into()));
        }
        let refund = session.refund_transaction().clone();
        session
            .verify_key_spend(&refund)
            .map_err(|err| PayError::Failed(format!("the presigned refund does not verify: {err}")))?;

        let held = self.orders().update(&order.id, |stored| {
            if let Some(Value::Object(terms)) = stored.escrow.as_mut() {
                terms.insert("outpoint".into(), json!(outpoint.to_string()));
                terms.insert("timeoutHeight".into(), json!(timeout_height));
                terms.insert("refundTx".into(), json!(serialize_hex(&refund)));
                terms.insert("refundTxid".into(), json!(refund.compute_txid().to_string()));
                terms.insert("fundingTx".into(), json!(serialize_hex(&tx)));
                terms.insert("refundHeld".into(), json!(true));
                terms.insert("safeToSend".into(), json!(true));
            }
        });
        match held {
            Ok(true) => {}
            Ok(false) => return Err(PayError::NotFound),
            Err(err) => return Err(PayError::Failed(format!("the refund could not be saved, nothing was sent: {err}"))),
        }
        self.broadcast_funding(&order, tx, funding, Some(fee_rate))
    }

    fn broadcast_funding(
        &self,
        order: &BuyerOrder,
        tx: Transaction,
        funding: u64,
        fee_rate: Option<FeeRate>,
    ) -> Result<Value, PayError> {
        self.chain.broadcast(&tx).map_err(PayError::Chain)?;
        let txid = tx.compute_txid().to_string();
        self.after_broadcast(&order.id, tx.clone(), &txid)?;
        let mut value = self.paid_value(&order.id, &tx, funding, fee_rate.unwrap_or(FeeRate::ZERO));
        value["refundHeld"] = json!(true);
        Ok(value)
    }

    fn order_address(&self, terms: Option<&Value>, key: &str) -> Result<Address, PayError> {
        let text = terms
            .and_then(|terms| terms.get(key))
            .and_then(Value::as_str)
            .ok_or_else(|| PayError::Invalid(format!("This order has no {} address.", self.network)))?;
        address_for(text, self.network).map_err(PayError::Invalid)
    }

    fn pay_fee_rate(&self) -> Result<FeeRate, PayError> {
        let rate = self.chain.fee_rate().map_err(PayError::Chain)?;
        if !rate.is_finite() || rate > MAX_FEE_RATE {
            return Err(PayError::Chain(format!(
                "The fee estimate ({rate} sat/vB) is above {MAX_FEE_RATE} sat/vB; nothing was sent."
            )));
        }
        let rate = rate.max(MIN_FEE_RATE);
        Ok(FeeRate::from_sat_per_kwu((rate * 250.0).ceil() as u64))
    }

    /// Build and sign a transaction with exactly one recipient output.
    fn sign_payment(&self, address: &Address, sats: u64, fee_rate: FeeRate) -> Result<Transaction, PayError> {
        let mut wallet = self.buyer_wallet();
        let untrusted = wallet.untrusted_pending();
        let mut builder = wallet.wallet.build_tx();
        builder
            .add_recipient(address.script_pubkey(), Amount::from_sat(sats))
            .fee_rate(fee_rate);
        for outpoint in untrusted {
            builder.add_unspendable(outpoint);
        }
        let mut psbt = builder.finish().map_err(|err| {
            PayError::Invalid(format!("The node wallet cannot pay {sats} sats: {err}"))
        })?;
        let finalized = wallet
            .sign(&mut psbt)
            .map_err(|err| PayError::Failed(err.to_string()))?;
        if !finalized {
            return Err(PayError::Failed("the payment could not be fully signed".into()));
        }
        let tx = psbt.extract_tx().map_err(|err| PayError::Failed(err.to_string()))?;
        let foreign = tx
            .output
            .iter()
            .filter(|out| !wallet.wallet.is_mine(out.script_pubkey.clone()))
            .count();
        if foreign != 1 || !tx.output.iter().any(|out| out.script_pubkey == address.script_pubkey()) {
            return Err(PayError::Failed("a payment must pay exactly one order".into()));
        }
        wallet.persist().map_err(|err| PayError::Failed(err.to_string()))?;
        Ok(tx)
    }

    fn after_broadcast(&self, id: &str, tx: Transaction, txid: &str) -> Result<(), PayError> {
        {
            let mut wallet = self.buyer_wallet();
            wallet.wallet.apply_unconfirmed_txs([(tx, now_secs())]);
            if let Err(err) = wallet.persist() {
                eprintln!("ob-node: wallet state after {txid}: {err}");
            }
        }
        self.orders()
            .update(id, |order| order.pay_txid = Some(txid.to_string()))
            .map_err(|err| PayError::Failed(format!("sent {txid}, but the order could not be updated: {err}")))?;
        Ok(())
    }

    fn paid_value(&self, id: &str, tx: &Transaction, sats: u64, fee_rate: FeeRate) -> Value {
        let fee = self.buyer_wallet().wallet.calculate_fee(tx).map(|fee| fee.to_sat()).ok();
        json!({
            "orderId": id,
            "network": self.network.as_str(),
            "txid": tx.compute_txid().to_string(),
            "amountSats": sats,
            "feeSats": fee,
            "feeRateSatVb": fee_rate.to_sat_per_kwu() as f64 / 250.0,
            "payjoin": false,
        })
    }
}
