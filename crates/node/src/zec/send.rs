//! Sending Zcash from the node wallet to an address the user names.
//!
//! The same two steps as Monero ([`crate::xmr::send`]), and the same payment
//! as an order ([`super::pay`]): `POST /zec/wallet/send/quote`
//! `{address, amountZatoshi}` prices one Ironwood payment with ZIP 317, and
//! `POST /zec/wallet/send` `{quote, passphrase, confirm: true}` proves it and
//! broadcasts it. Spendable notes are those [`openbazaar_zec::CONFIRMATIONS`]
//! deep, the same bar an order payment uses. There is no new cryptography
//! and no send on mainnet.

use openbazaar_zec::amount::format_zec;
use openbazaar_zec::{quote_payment, sign_payment, SpendKey, ZecNetwork};
use serde_json::{json, Map, Value};

use crate::pay_order::PayError;
use crate::spend_tickets::{FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_keys;
use crate::wallet_send::QUOTE_USED;
use crate::Node;

use super::pay::{send_error, shown};

const TESTNET_ONLY: &str = "Sending Zcash from the wallet is testnet only.";

/// What one Zcash send to an address the user names may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ZecSendTerms {
    address: String,
    amount: u64,
    fee: u64,
    fee_cap: u64,
}

impl ZecSendTerms {
    fn value(&self, network: &str) -> Value {
        let total = self.amount.saturating_add(self.fee);
        json!({
            "network": network,
            "address": self.address,
            "amountZatoshi": self.amount.to_string(),
            "amountZec": format_zec(self.amount),
            "feeZatoshi": self.fee.to_string(),
            "feeZec": format_zec(self.fee),
            "feeCapZatoshi": self.fee_cap.to_string(),
            "feeCapZec": format_zec(self.fee_cap),
            "totalZatoshi": total.to_string(),
            "totalZec": format_zec(total),
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

fn amount(map: &Map<String, Value>) -> Result<u64, PayError> {
    map.get("amountZatoshi")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|amount| *amount > 0)
        .ok_or_else(|| {
            PayError::Invalid(
                "Send the amount in zatoshi as a whole number above zero, as a string (1 ZEC is \"100000000\")."
                    .into(),
            )
        })
}

impl Node {
    fn zec_send_network(&self) -> Result<ZecNetwork, PayError> {
        let network = self.zec.zec_network();
        if network != ZecNetwork::Testnet {
            return Err(PayError::Invalid(TESTNET_ONLY.into()));
        }
        Ok(network)
    }

    /// `POST /zec/wallet/send/quote`.
    pub(super) fn zec_send_quote(&self, map: &Map<String, Value>) -> Result<Value, PayError> {
        let network = self.zec_send_network()?;
        self.lock.spend_key()?;
        self.lock.touch();
        if self.zec.chain().is_none() {
            return Err(PayError::Invalid("Zcash is off on this node.".into()));
        }
        let address = map.get("address").and_then(Value::as_str).unwrap_or_default();
        let amount = amount(map)?;
        let wallet = self
            .zec
            .own()
            .wallet
            .clone()
            .ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let quote = quote_payment(&wallet, address, amount).map_err(send_error)?;
        let terms = ZecSendTerms {
            address: quote.destination,
            amount,
            fee: quote.fee,
            fee_cap: (u128::from(quote.fee) * u128::from(FEE_CAP_PERCENT)).div_ceil(100) as u64,
        };
        let mut value = terms.value(network.as_str());
        value["quote"] = json!(self.lock.zec_send_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// `POST /zec/wallet/send`, after the passphrase: spend `quote` once.
    pub(super) fn zec_send_quoted(&self, quote: &str) -> Result<Value, PayError> {
        let network = self.zec_send_network()?;
        let terms = self
            .lock
            .zec_send_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| PayError::Conflict(QUOTE_USED.into()))?;
        let chain = self
            .zec
            .chain()
            .cloned()
            .ok_or_else(|| PayError::Invalid("Zcash is off on this node.".into()))?;
        let wallet = self
            .zec
            .own()
            .wallet
            .clone()
            .ok_or(PayError::Locked(crate::wallet_lock::State::Locked.as_str()))?;
        let spend = {
            let key = self.lock.spend_key()?;
            let dir = self.buyer_wallet().dir.clone();
            let seed = wallet_keys::spend_seed(&dir, &key, self.network).map_err(|err| PayError::Failed(err.to_string()))?;
            SpendKey::from_seed(seed.zip32_seed()).map_err(send_error)?
        };
        let signed = sign_payment(&wallet, &spend, &terms.address, terms.amount, terms.fee_cap).map_err(send_error)?;
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
        self.lock.touch();
        eprintln!("ob-node: sent Zcash tx {txid} from the node wallet");
        let mut value = ZecSendTerms { fee: signed.fee, ..terms }.value(network.as_str());
        value["txid"] = json!(txid);
        Ok(value)
    }
}
