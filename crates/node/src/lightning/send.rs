//! Paying an invoice the user pastes, and withdrawing LDK's on-chain funds
//! to an address the user names: the same two steps as [`crate::wallet_send`].
//!
//! | route | body | answer |
//! |---|---|---|
//! | `POST /ln/send/quote` | `{invoice}` | `{quote, paymentHash, amountSats, feeCapSats, payee, description, ...}` |
//! | `POST /ln/send` | `{quote, passphrase, confirm: true}` | the terms with `status` |
//! | `POST /ln/withdraw/quote` | `{address, amountSats}` or `{address, max: true}` | `{quote, address, amountSats, max, ...}` |
//! | `POST /ln/withdraw` | `{quote, passphrase, confirm: true}` | the terms with `txid` |
//!
//! An invoice must carry an amount, be unexpired, be for the Lightning
//! node's network, and not be this node's own; routing fees are capped as
//! for orders ([`fee_cap`]). A withdrawal goes to an address on the
//! Lightning node's network, which may differ from the bitcoin wallet's
//! (Mutinynet). LDK picks its on-chain fee when it sends, so a quote cannot
//! show it: an amount is sent in full with the fee on top, and send max pays
//! the fee out of the balance and keeps LDK's anchor reserve. Closing a
//! channel returns its funds to LDK's on-chain balance, from where they are
//! withdrawn the same way.

use std::str::FromStr;

use bitcoin::Address;
use openbazaar_lightning::{decode_invoice, LnError, Outgoing};
use serde_json::{json, Map, Value};

use crate::buyer_orders::now_secs;
use crate::pay_order::PayError;
use crate::spend_tickets::TICKET_TTL;
use crate::wallet_send::QUOTE_USED;
use crate::Node;

use super::pay::fee_cap;

/// What one Lightning send may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LnSendTerms {
    Invoice {
        invoice: String,
        payment_hash: String,
        amount_msat: u64,
        fee_cap_msat: u64,
        payee: String,
        description: String,
    },
    Withdraw {
        address: String,
        amount_sats: Option<u64>,
        spendable_sats: u64,
    },
}

impl LnSendTerms {
    fn value(&self, network: &str) -> Value {
        match self {
            Self::Invoice {
                payment_hash,
                amount_msat,
                fee_cap_msat,
                payee,
                description,
                ..
            } => json!({
                "network": network,
                "paymentHash": payment_hash,
                "amountMsat": amount_msat,
                "amountSats": amount_msat.div_ceil(1_000),
                "feeCapMsat": fee_cap_msat,
                "feeCapSats": fee_cap_msat.div_ceil(1_000),
                "payee": payee,
                "description": description,
                "expiresInSecs": TICKET_TTL.as_secs(),
            }),
            Self::Withdraw {
                address,
                amount_sats,
                spendable_sats,
            } => json!({
                "network": network,
                "address": address,
                "amountSats": amount_sats.unwrap_or(*spendable_sats),
                "max": amount_sats.is_none(),
                "expiresInSecs": TICKET_TTL.as_secs(),
            }),
        }
    }
}

fn ln_failed(err: LnError) -> PayError {
    match err {
        LnError::Invalid(message) | LnError::Insufficient(message) => PayError::Invalid(message),
        err => PayError::Chain(err.to_string()),
    }
}

impl Node {
    fn ln_network(&self, rail: &dyn openbazaar_lightning::LightningRail) -> bitcoin::Network {
        self.ln
            .config()
            .map(|config| config.network)
            .unwrap_or(rail.info().network)
    }

    /// `POST /ln/send/quote`.
    pub(super) fn ln_send_quote(&self, map: &Map<String, Value>) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let rail = self.ln_rail_or_refuse()?;
        let invoice = map
            .get("invoice")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let invoice = invoice
            .strip_prefix("lightning:")
            .unwrap_or(invoice)
            .to_string();
        let info = decode_invoice(&invoice, self.ln_network(rail.as_ref())).map_err(ln_failed)?;
        if info.expires_at <= now_secs() {
            return Err(PayError::Invalid(
                "That invoice has expired. Ask for a new one.".into(),
            ));
        }
        if info.payee == rail.info().node_id {
            return Err(PayError::Invalid(
                "This node issued that invoice. Pay it from another Lightning wallet.".into(),
            ));
        }
        let amount_msat = info
            .amount_msat
            .filter(|amount| *amount > 0)
            .ok_or_else(|| {
                PayError::Invalid("That invoice has no amount. Ask for one with an amount.".into())
            })?;
        let fee_cap_msat = fee_cap(amount_msat);
        let spendable = rail.balances().lightning_sats.saturating_mul(1_000);
        if spendable < amount_msat.saturating_add(fee_cap_msat) {
            return Err(PayError::Invalid(format!(
                "Not enough funds: the Lightning balance is {} sats; this invoice needs {} sats plus up to {} sats in routing fees.",
                spendable / 1_000,
                amount_msat.div_ceil(1_000),
                fee_cap_msat.div_ceil(1_000),
            )));
        }
        let terms = LnSendTerms::Invoice {
            invoice,
            payment_hash: info.payment_hash,
            amount_msat,
            fee_cap_msat,
            payee: info.payee,
            description: info.description,
        };
        self.lock.touch();
        let mut value = terms.value(self.ln_network_name());
        value["quote"] = json!(self.lock.ln_send_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// `POST /ln/withdraw/quote`.
    pub(super) fn ln_withdraw_quote(&self, map: &Map<String, Value>) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        let rail = self.ln_rail_or_refuse()?;
        let network = self.ln_network(rail.as_ref());
        let text = map
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let address = Address::from_str(text)
            .map_err(|_| PayError::Invalid("That is not a bitcoin address.".into()))?
            .require_network(network)
            .map_err(|_| {
                PayError::Invalid(format!(
                    "That address is not on {}, the Lightning node's network.",
                    self.ln_network_name()
                ))
            })?;
        let max = map.get("max") == Some(&Value::Bool(true));
        let amount = map
            .get("amountSats")
            .map(|value| value.as_u64().filter(|sats| *sats > 0));
        let amount_sats = match (max, amount) {
            (true, None) => None,
            (false, Some(Some(sats))) => Some(sats),
            (true, Some(_)) => {
                return Err(PayError::Invalid(
                    "Send either an amount or max, not both.".into(),
                ))
            }
            _ => {
                return Err(PayError::Invalid(
                    "Send the amount in whole sats above zero, or max: true.".into(),
                ))
            }
        };
        let spendable_sats = rail.balances().onchain_spendable_sats;
        let dust = address.script_pubkey().minimal_non_dust().to_sat();
        if amount_sats.is_some_and(|sats| sats < dust) {
            return Err(PayError::Invalid(format!(
                "That is below the {dust} sat dust limit for this address."
            )));
        }
        if spendable_sats == 0 || amount_sats.is_some_and(|sats| sats >= spendable_sats) {
            return Err(PayError::Invalid(format!(
                "Not enough funds: the Lightning node has {spendable_sats} sats on chain, and the network fee is paid on top of the amount."
            )));
        }
        let terms = LnSendTerms::Withdraw {
            address: address.to_string(),
            amount_sats,
            spendable_sats,
        };
        self.lock.touch();
        let mut value = terms.value(self.ln_network_name());
        value["quote"] = json!(self.lock.ln_send_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// `POST /ln/send` and `POST /ln/withdraw`, after the passphrase: spend `quote` once.
    pub(super) fn ln_send_quoted(&self, quote: &str, withdraw: bool) -> Result<Value, PayError> {
        let terms = self
            .lock
            .ln_send_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| PayError::Conflict(QUOTE_USED.into()))?;
        let rail = self.ln_rail_or_refuse()?;
        let mut value = terms.value(self.ln_network_name());
        match &terms {
            LnSendTerms::Invoice {
                invoice,
                payment_hash,
                fee_cap_msat,
                ..
            } if !withdraw => {
                let hash = rail.pay(invoice, *fee_cap_msat).map_err(ln_failed)?;
                eprintln!("ob-node: paid Lightning invoice {payment_hash}");
                let status = match rail.outgoing(&hash) {
                    Outgoing::Paid { fee_msat, .. } => {
                        value["feeSats"] = json!(fee_msat.div_ceil(1_000));
                        "paid"
                    }
                    Outgoing::Failed => "failed",
                    Outgoing::Pending | Outgoing::Unknown => "pending",
                };
                value["status"] = json!(status);
            }
            LnSendTerms::Withdraw {
                address,
                amount_sats,
                ..
            } if withdraw => {
                let txid = match amount_sats {
                    Some(sats) => rail.send_onchain(address, *sats),
                    None => rail.sweep_onchain(address),
                }
                .map_err(ln_failed)?;
                eprintln!("ob-node: withdrew Lightning on-chain funds in {txid}");
                value["txid"] = json!(txid);
            }
            _ => return Err(PayError::Conflict(QUOTE_USED.into())),
        }
        self.lock.touch();
        Ok(value)
    }
}
