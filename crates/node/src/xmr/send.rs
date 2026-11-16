//! Sending Monero from the node wallet to an address the user names.
//!
//! The same two steps as bitcoin ([`crate::wallet_send`]):
//! `POST /xmr/wallet/send/quote` `{address, amountPiconero}` prices the send
//! and answers a one-time quote, and `POST /xmr/wallet/send`
//! `{quote, passphrase, confirm: true}` signs and broadcasts it. The fee may
//! rise to the quoted fee plus half, no further. There is no "send max":
//! the fee depends on the inputs, so the user names the amount. Integrated
//! addresses are refused, as for orders.

use openbazaar_xmr::amount::format_xmr;
use openbazaar_xmr::{address_for, quote_payment};
use serde_json::{json, Map, Value};

use crate::pay_order::PayError;
use crate::spend_tickets::{FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_send::QUOTE_USED;
use crate::Node;

use super::pay::send_error;

/// What one Monero send may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XmrSendTerms {
    address: String,
    amount: u64,
    fee: u64,
    fee_cap: u64,
}

impl XmrSendTerms {
    fn value(&self, network: &str) -> Value {
        let total = self.amount.saturating_add(self.fee);
        json!({
            "network": network,
            "address": self.address,
            "amountPiconero": self.amount.to_string(),
            "amountXmr": format_xmr(self.amount),
            "feePiconero": self.fee.to_string(),
            "feeXmr": format_xmr(self.fee),
            "feeCapPiconero": self.fee_cap.to_string(),
            "feeCapXmr": format_xmr(self.fee_cap),
            "totalPiconero": total.to_string(),
            "totalXmr": format_xmr(total),
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

fn amount(map: &Map<String, Value>) -> Result<u64, PayError> {
    map.get("amountPiconero")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|amount| *amount > 0)
        .ok_or_else(|| {
            PayError::Invalid(
                "Send the amount in piconero as a whole number above zero, as a string (1 XMR is \"1000000000000\").".into(),
            )
        })
}

impl Node {
    /// `POST /xmr/wallet/send/quote`.
    pub(super) fn xmr_send_quote(&self, map: &Map<String, Value>) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        let chain = self.xmr_chain_or_refuse()?;
        let network = self.xmr.xmr_network();
        let address = map
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let destination = address_for(address, network).map_err(PayError::Invalid)?;
        let amount = amount(map)?;
        let fee_rate = chain.fee_rate().map_err(PayError::Chain)?;
        let wallet = self
            .xmr
            .buyer()
            .wallet
            .clone()
            .ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let quote = quote_payment(&wallet, &destination, amount, fee_rate).map_err(send_error)?;
        let terms = XmrSendTerms {
            address: destination.to_string(),
            amount,
            fee: quote.fee,
            fee_cap: (u128::from(quote.fee) * u128::from(FEE_CAP_PERCENT)).div_ceil(100) as u64,
        };
        let mut value = terms.value(network.as_str());
        value["quote"] = json!(self.lock.xmr_send_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// `POST /xmr/wallet/send`, after the passphrase: spend `quote` once.
    pub(super) fn xmr_send_quoted(&self, quote: &str) -> Result<Value, PayError> {
        let terms = self
            .lock
            .xmr_send_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| PayError::Conflict(QUOTE_USED.into()))?;
        let chain = self.xmr_chain_or_refuse()?;
        let network = self.xmr.xmr_network();
        let destination = address_for(&terms.address, network).map_err(PayError::Invalid)?;
        let (txid, fee) = self.xmr_spend(&chain, &destination, terms.amount, terms.fee_cap)?;
        self.lock.touch();
        eprintln!("ob-node: sent Monero tx {txid} from the node wallet");
        let mut value = XmrSendTerms { fee, ..terms }.value(network.as_str());
        value["txid"] = json!(txid);
        Ok(value)
    }
}
