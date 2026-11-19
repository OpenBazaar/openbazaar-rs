//! Paying a filing-fee request: a quote, then one passphrase.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `POST /orders/{id}/dispute-fee/quote` | unlocked | none | `{quote, ...terms}` |
//! | `POST /orders/{id}/dispute-fee` | unlocked | `{quote, passphrase, confirm: true}` | `{...terms, txid or paymentHash}` |
//!
//! The vendor's routes are the same under `/escrow/sales/{id}/`. The quote
//! fixes the request it pays, the amount, and the rail: Lightning when the
//! request carries an invoice this node can pay, else an on-chain payment
//! to the request's script at a fee capped like any send. A quote is good
//! for one payment whether or not it goes through, and nothing is quoted
//! for a fee already paid or a letter already denied.
//!
//! The on-chain payment is recorded before it is broadcast, so a crash in
//! between cannot lead to a second payment; a broadcast that fails clears it
//! ([`super::watch`]), and a payment made after a cleared one spends one of
//! its coins.

use bitcoin::{Address, FeeRate, ScriptBuf, Transaction};
use openbazaar_escrow::Party;
use serde_json::{json, Value};

use super::{FiledEscrow, FilerFee, Paid, Payment};
use crate::buyer_orders::now_secs;
use crate::escrow_buyer::escrow_id_of;
use crate::escrow_mail::{hex, id_hex, parse_id, EscrowId};
use crate::http::{json_body, local_guard, Client};
use crate::lightning::FeePaid;
use crate::lock_http::pay_error;
use crate::pay_order::{psbt_fee, PayError};
use crate::spend_tickets::{FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_send::{confirmed_response, paid_to, SendShape, QUOTE_USED};
use crate::Node;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rail {
    Lightning {
        bolt11: String,
        payment_hash: String,
        fee_cap_msat: u64,
    },
    OnChain {
        address: String,
        recipient: ScriptBuf,
        fee_sats: u64,
        fee_cap_sats: u64,
        fee_rate: FeeRate,
    },
}

/// What one filing-fee payment is allowed to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisputeFeeTerms {
    pub escrow: EscrowId,
    pub party: Party,
    pub request: [u8; 32],
    pub amount_sats: u64,
    pub rail: Rail,
}

impl DisputeFeeTerms {
    fn value(&self, network: &str) -> Value {
        let mut value = json!({
            "escrowId": id_hex(&self.escrow),
            "network": network,
            "request": hex(&self.request),
            "amountSats": self.amount_sats,
            "expiresInSecs": TICKET_TTL.as_secs(),
        });
        match &self.rail {
            Rail::Lightning {
                payment_hash,
                fee_cap_msat,
                ..
            } => {
                value["rail"] = json!("ln");
                value["paymentHash"] = json!(payment_hash);
                value["feeCapSats"] = json!(fee_cap_msat.div_ceil(1_000));
            }
            Rail::OnChain {
                address,
                fee_sats,
                fee_cap_sats,
                ..
            } => {
                value["rail"] = json!("onchain");
                value["address"] = json!(address);
                value["feeSats"] = json!(fee_sats);
                value["feeCapSats"] = json!(fee_cap_sats);
                value["totalSats"] = json!(self.amount_sats.saturating_add(*fee_sats));
            }
        }
        value
    }
}

fn conflict(message: &str) -> PayError {
    PayError::Conflict(message.into())
}

impl Node {
    /// The escrow a route names: an order id for the buyer, the escrow id
    /// for the vendor's sale.
    fn fee_escrow_id(&self, party: Party, id: &str) -> Result<EscrowId, PayError> {
        match party {
            Party::Buyer => self
                .orders()
                .get(id)
                .and_then(|order| escrow_id_of(order.escrow.as_ref()?)),
            Party::Vendor => parse_id(id),
        }
        .ok_or(PayError::NotFound)
    }

    fn filed_escrow(&self, party: Party, id: &EscrowId) -> Result<FiledEscrow, PayError> {
        match party {
            Party::Buyer => self.buyer_filed_escrow(id),
            Party::Vendor => self.vendor_filed_escrow(id),
        }
        .map_err(|_| PayError::NotFound)
    }

    /// A payment this node made that still counts: a Lightning payment that
    /// failed does not.
    fn standing_payment(&self, fee: &FilerFee) -> Option<Payment> {
        let payment = fee.payment.clone()?;
        match &payment.paid {
            Paid::Lightning { payment_hash } if self.ln_fee_paid(payment_hash) == FeePaid::Failed => None,
            _ => Some(payment),
        }
    }

    /// The newest request, if it can still be paid.
    fn payable(&self, fee: &FilerFee) -> Result<(openbazaar_protocol::FeeRequest, [u8; 32]), PayError> {
        if fee.recused.is_some() {
            return Err(conflict("The moderator recused itself from this dispute. Nothing is owed."));
        }
        if fee.denied.is_some() {
            return Err(conflict("The moderator denied this dispute. A denial is final."));
        }
        if fee.receipt.is_some() {
            return Err(conflict("The filing fee is paid; the moderator signed a receipt."));
        }
        if self.standing_payment(fee).is_some() {
            return Err(conflict(
                "The filing fee is paid. The moderator's receipt is on its way.",
            ));
        }
        let (request, digest) = fee
            .latest()
            .ok_or_else(|| conflict("The moderator has not asked for a filing fee yet."))?;
        let tip = self.chain.tip_height().map_err(PayError::Chain)?;
        if request.valid_until_height < tip {
            return Err(conflict(
                "The moderator's request expired. Send the dispute again for a fresh one.",
            ));
        }
        Ok((request, digest))
    }

    /// `POST .../dispute-fee/quote`: the exact payment the party is asked to confirm.
    pub(crate) fn quote_dispute_fee(&self, party: Party, id: &str) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        let escrow_id = self.fee_escrow_id(party, id)?;
        self.filed_escrow(party, &escrow_id)?;
        let fee = self.filer_fee(party, &escrow_id).map_err(PayError::Failed)?;
        let (request, digest) = self.payable(&fee)?;
        let amount = request.amount_sats;
        let lightning = request.invoice.as_ref().and_then(|invoice| {
            let hash = hex(&invoice.payment_hash);
            let cap = self
                .ln_can_pay_fee(&invoice.network, &invoice.bolt11, &hash, amount)
                .ok()?;
            Some(Rail::Lightning {
                bolt11: invoice.bolt11.clone(),
                payment_hash: hash,
                fee_cap_msat: cap,
            })
        });
        let rail = match lightning {
            Some(rail) => rail,
            None => {
                let linked_to = self.escrow_change_scope(party, &escrow_id);
                self.quote_on_chain(&ScriptBuf::from_bytes(request.script.clone()), amount, linked_to)?
            }
        };
        let terms = DisputeFeeTerms {
            escrow: escrow_id,
            party,
            request: digest,
            amount_sats: amount,
            rail,
        };
        let mut value = terms.value(self.network.as_str());
        value["quote"] = json!(self.lock.dispute_fee_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// The escrow output a buyer's change from funding it is linked to. Its
    /// filing fee may spend that change; the result is linked to both.
    fn escrow_change_scope(&self, party: Party, id: &EscrowId) -> Option<ScriptBuf> {
        if party != Party::Buyer {
            return None;
        }
        let outpoint = self.buyer_filed_escrow(id).ok()?.outpoint;
        let wallet = self.buyer_wallet();
        let funding = wallet.wallet.get_tx(outpoint.txid)?;
        let script = funding.tx_node.tx.output.get(outpoint.vout as usize)?.script_pubkey.clone();
        Some(script)
    }

    fn quote_on_chain(&self, recipient: &ScriptBuf, amount: u64, linked_to: Option<ScriptBuf>) -> Result<Rail, PayError> {
        let address = Address::from_script(recipient, self.network.bitcoin())
            .map_err(|_| conflict("The moderator's script has no address, and this node cannot pay its invoice."))?;
        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let shape = SendShape {
            linked_to,
            ..SendShape::default()
        };
        let (psbt, _) = self.build_send_from(&address, Some(amount), fee_rate, false, &shape)?;
        let fee = psbt_fee(&psbt)?;
        Ok(Rail::OnChain {
            address: address.to_string(),
            recipient: recipient.clone(),
            fee_sats: fee,
            fee_cap_sats: (fee * FEE_CAP_PERCENT).div_ceil(100),
            fee_rate,
        })
    }

    /// `POST .../dispute-fee`, after the passphrase: pay `quote` once.
    pub(crate) fn pay_dispute_fee(&self, party: Party, id: &str, quote: &str) -> Result<Value, PayError> {
        let terms = self
            .lock
            .dispute_fee_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| conflict(QUOTE_USED))?;
        let escrow_id = self.fee_escrow_id(party, id)?;
        if terms.escrow != escrow_id || terms.party != party {
            return Err(conflict("That quote is for another dispute. Nothing was paid."));
        }
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let mut fee = self.filer_fee(party, &escrow_id).map_err(PayError::Failed)?;
        let (_, digest) = self.payable(&fee)?;
        if digest != terms.request {
            return Err(conflict(
                "The moderator sent a newer request. Nothing was paid; review the fee again.",
            ));
        }
        let mut value = terms.value(self.network.as_str());
        match &terms.rail {
            Rail::Lightning {
                bolt11,
                payment_hash,
                fee_cap_msat,
            } => {
                self.ln_pay_fee(bolt11, *fee_cap_msat).map_err(PayError::Chain)?;
                fee.payment = Some(Payment {
                    request: digest,
                    paid: Paid::Lightning {
                        payment_hash: payment_hash.clone(),
                    },
                    amount_sats: terms.amount_sats,
                    fee_sats: fee_cap_msat.div_ceil(1_000),
                    tx: None,
                    unseen: None,
                });
                self.save_filer_fee(party, &escrow_id, &fee).map_err(PayError::Failed)?;
            }
            Rail::OnChain {
                recipient,
                fee_rate,
                fee_cap_sats,
                ..
            } => {
                let (txid, fee_sats) =
                    self.pay_fee_on_chain(party, &escrow_id, &mut fee, &terms, recipient, *fee_rate, *fee_cap_sats)?;
                value["txid"] = json!(txid);
                value["feeSats"] = json!(fee_sats);
            }
        }
        self.lock.touch();
        Ok(value)
    }

    #[allow(clippy::too_many_arguments)]
    fn pay_fee_on_chain(
        &self,
        party: Party,
        id: &EscrowId,
        fee: &mut FilerFee,
        terms: &DisputeFeeTerms,
        recipient: &ScriptBuf,
        fee_rate: FeeRate,
        fee_cap_sats: u64,
    ) -> Result<(String, u64), PayError> {
        let _spending = self.spending.lock().unwrap_or_else(|err| err.into_inner());
        let shape = SendShape {
            lock_height: Some(self.chain.tip_height().map_err(PayError::Chain)?),
            spend: self.cleared_coins(fee),
            linked_to: self.escrow_change_scope(party, id),
        };
        let (tx, network_fee) = self.signed_send(recipient, terms.amount_sats, fee_rate, fee_cap_sats, &shape)?;
        let txid = tx.compute_txid();
        let vout = tx
            .output
            .iter()
            .position(|out| &out.script_pubkey == recipient)
            .unwrap_or_default() as u32;
        fee.payment = Some(Payment {
            request: terms.request,
            paid: Paid::OnChain { txid, vout },
            amount_sats: terms.amount_sats,
            fee_sats: network_fee,
            tx: Some(tx.clone()),
            unseen: None,
        });
        self.save_filer_fee(party, id, fee).map_err(PayError::Failed)?;
        if let Err(err) = self.chain.broadcast(&tx) {
            fee.clear_payment();
            self.save_filer_fee(party, id, fee).map_err(PayError::Failed)?;
            return Err(PayError::Chain(err));
        }
        {
            let mut wallet = self.buyer_wallet();
            wallet.wallet.apply_unconfirmed_txs([(tx, now_secs())]);
            if let Err(err) = wallet.persist() {
                eprintln!("ob-node: wallet state after {txid}: {err}");
            }
        }
        eprintln!("ob-node: paid a dispute filing fee in {txid}");
        Ok((txid.to_string(), network_fee))
    }

    /// A signed payment of exactly `amount` to `recipient` from the node
    /// wallet, at no more than `fee_cap_sats` of network fee, and that fee.
    /// The caller holds the wallet's spending lock.
    pub(crate) fn signed_send(
        &self,
        recipient: &ScriptBuf,
        amount: u64,
        fee_rate: FeeRate,
        fee_cap_sats: u64,
        shape: &SendShape,
    ) -> Result<(Transaction, u64), PayError> {
        let address =
            Address::from_script(recipient, self.network.bitcoin()).map_err(|err| PayError::Failed(err.to_string()))?;
        let (mut psbt, _) = self.build_send_from(&address, Some(amount), fee_rate, false, shape)?;
        if paid_to(&psbt, recipient) != Some(amount) {
            return Err(conflict(
                "The wallet's balance changed since the quote. Nothing was paid; review it again.",
            ));
        }
        let network_fee = psbt_fee(&psbt)?;
        if network_fee > fee_cap_sats {
            return Err(PayError::Conflict(format!(
                "The network fee is now {network_fee} sats, above the {fee_cap_sats} sats you confirmed. Nothing was paid; review it again."
            )));
        }
        let wallet = self.buyer_wallet();
        let seed = self.spend_seed(&wallet)?;
        if !wallet
            .sign(&mut psbt, &seed)
            .map_err(|err| PayError::Failed(err.to_string()))?
        {
            return Err(PayError::Failed("the payment could not be fully signed".into()));
        }
        let tx = psbt.extract_tx().map_err(|err| PayError::Failed(err.to_string()))?;
        Ok((tx, network_fee))
    }
}

pub(crate) fn quote_response(
    node: &Node,
    client: &Client,
    head: &str,
    party: Party,
    id: &str,
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    match node.quote_dispute_fee(party, id) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => pay_error(err, origin),
    }
}

pub(crate) fn pay_response(
    node: &Node,
    client: &Client,
    head: &str,
    party: Party,
    id: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    confirmed_response(node, body, origin, |quote| node.pay_dispute_fee(party, id, quote))
}
