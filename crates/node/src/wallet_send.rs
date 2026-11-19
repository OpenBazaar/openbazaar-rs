//! Sending from the node's bitcoin wallet to an address the user names.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `POST /wallet/send/quote` | unlocked | `{address, amountSats}` or `{address, max: true}`, `linkCoins?` | `{quote, ...terms}` |
//! | `POST /wallet/send` | unlocked | `{quote, passphrase, confirm: true}` | `{txid, ...terms}` |
//!
//! The quote fixes the recipient, the amount (with `max`, everything this
//! node will spend, less the fee), the fee rate, and a fee cap. The send
//! needs the passphrase again even while the wallet is unlocked, and
//! `"confirm": true` on every network. A quote is good for one send whether
//! or not it goes through, so a second click or a replayed request sends
//! nothing. The transaction pays exactly the quoted amount to the quoted
//! address at the quoted rate; every other output is the wallet's own.
//! Paired devices may call neither route ([`crate::remote::policy`]).
//!
//! The node logs a send's txid, never its address or amount.

use bdk_wallet::coin_selection::InsufficientFunds;
use bdk_wallet::error::CreateTxError;
use bitcoin::absolute::LockTime;
use bitcoin::{Address, Amount, FeeRate, OutPoint, Psbt, ScriptBuf};
use serde_json::{json, Value};

use crate::buyer_orders::now_secs;
use crate::http::{json_body, json_error, local_guard, Client};
use crate::lock_http::{lock_error, pay_error};
use crate::network::{address_for, NetworkMode};
use crate::pay_order::{psbt_fee, PayError};
use crate::spend_tickets::{FEE_CAP_PERCENT, TICKET_TTL};
use crate::Node;

const MAX_BODY: usize = 4 * 1024;
/// A quote that was used, expired, or never issued. Nothing was sent.
pub(crate) const QUOTE_USED: &str =
    "That quote is used or expired. Nothing was sent; review the send again.";
/// 21 million bitcoin, in sats.
const MAX_SATS: u64 = 2_100_000_000_000_000;

/// What one send is allowed to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SendTerms {
    pub address: String,
    pub recipient: ScriptBuf,
    pub amount_sats: u64,
    pub max: bool,
    pub link_coins: bool,
    pub fee_sats: u64,
    pub fee_cap_sats: u64,
    pub fee_rate: FeeRate,
}

impl SendTerms {
    fn value(&self, network: &str) -> Value {
        json!({
            "network": network,
            "address": self.address,
            "amountSats": self.amount_sats,
            "max": self.max,
            "linkCoins": self.link_coins,
            "feeSats": self.fee_sats,
            "feeCapSats": self.fee_cap_sats,
            "feeRateSatVb": self.fee_rate.to_sat_per_kwu() as f64 / 250.0,
            "totalSats": self.amount_sats.saturating_add(self.fee_sats),
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

/// A quote request, checked.
struct Request {
    address: Address,
    /// `None`: send everything.
    amount: Option<u64>,
    link_coins: bool,
}

fn flag(map: &serde_json::Map<String, Value>, key: &str) -> Result<bool, PayError> {
    match map.get(key) {
        None => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(PayError::Invalid(format!("{key} must be true or false."))),
    }
}

fn parse_quote(body: &Value, network: NetworkMode) -> Result<Request, PayError> {
    let invalid = |message: &str| PayError::Invalid(message.into());
    let map = body
        .as_object()
        .ok_or_else(|| invalid("The request could not be read."))?;
    if map
        .keys()
        .any(|key| !["address", "amountSats", "max", "linkCoins"].contains(&key.as_str()))
    {
        return Err(invalid("That field is not part of a send."));
    }
    let text = map
        .get("address")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Err(invalid("Enter the address to send to."));
    }
    let address = address_for(text, network).map_err(PayError::Invalid)?;
    let max = flag(map, "max")?;
    let link_coins = flag(map, "linkCoins")?;
    let amount = match (max, map.get("amountSats")) {
        (true, None) => None,
        (true, Some(_)) => {
            return Err(invalid(
                "Send either an amount or the whole balance, not both.",
            ))
        }
        (false, None) => return Err(invalid("Enter an amount in sats.")),
        (false, Some(value)) => {
            let sats = value
                .as_u64()
                .filter(|sats| *sats > 0 && *sats <= MAX_SATS)
                .ok_or_else(|| invalid("The amount must be a whole number of sats above zero."))?;
            let dust = address.script_pubkey().minimal_non_dust().to_sat();
            if sats < dust {
                return Err(PayError::Invalid(format!(
                    "{sats} sats is below the dust limit for that address ({dust} sats); the network would not relay it."
                )));
            }
            Some(sats)
        }
    };
    Ok(Request {
        address,
        amount,
        link_coins,
    })
}

/// The sats `psbt` pays to `recipient`, when exactly one output does.
pub(crate) fn paid_to(psbt: &Psbt, recipient: &ScriptBuf) -> Option<u64> {
    let mut paying = psbt
        .unsigned_tx
        .output
        .iter()
        .filter(|out| &out.script_pubkey == recipient);
    match (paying.next(), paying.next()) {
        (Some(out), None) => Some(out.value.to_sat()),
        _ => None,
    }
}

/// Constraints on a send beyond its recipient and fee rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SendShape {
    /// The lock time, as a block height.
    pub lock_height: Option<u32>,
    /// Coins the send must spend.
    pub spend: Vec<OutPoint>,
    /// Another recipient whose linked change this send may spend.
    pub linked_to: Option<ScriptBuf>,
}

fn build_error(err: CreateTxError, held_back: u64) -> PayError {
    let mut message = match err {
        CreateTxError::CoinSelection(InsufficientFunds { needed, available }) => format!(
            "Not enough funds: this needs {} sats including the network fee, and the wallet can spend {} sats now.",
            needed.to_sat(),
            available.to_sat()
        ),
        CreateTxError::NoUtxosSelected => "The wallet has nothing it can spend now.".to_string(),
        CreateTxError::OutputBelowDustLimit(_) => {
            "After the network fee, the amount is below the dust limit; the network would not relay it.".to_string()
        }
        other => format!("The node wallet cannot build this send: {other}"),
    };
    if held_back > 0 {
        message.push_str(&format!(
            " {held_back} sats of change from earlier payments is held back so those payments stay unlinked on chain."
        ));
    }
    PayError::Invalid(message)
}

impl Node {
    /// An unsigned send of `amount` (or everything) to `address`, and the
    /// sats of linked change it left out.
    pub(crate) fn build_send(
        &self,
        address: &Address,
        amount: Option<u64>,
        fee_rate: FeeRate,
        link_coins: bool,
    ) -> Result<(Psbt, u64), PayError> {
        self.build_send_from(address, amount, fee_rate, link_coins, &SendShape::default())
    }

    /// [`Self::build_send`], with a lock time and coins it must spend.
    pub(crate) fn build_send_from(
        &self,
        address: &Address,
        amount: Option<u64>,
        fee_rate: FeeRate,
        link_coins: bool,
        shape: &SendShape,
    ) -> Result<(Psbt, u64), PayError> {
        let mut wallet = self.buyer_wallet();
        let recipient = address.script_pubkey();
        let mut allowed = vec![recipient.as_script()];
        allowed.extend(shape.linked_to.as_deref());
        let blocked = crate::coin_provenance::blocked_coins_among(&wallet.wallet, &allowed, link_coins);
        let held_back: u64 = wallet
            .wallet
            .list_unspent()
            .filter(|utxo| blocked.contains(&utxo.outpoint))
            .map(|utxo| utxo.txout.value.to_sat())
            .sum();
        let untrusted = wallet.untrusted_pending();
        let mut builder = wallet.wallet.build_tx();
        match amount {
            Some(sats) => {
                builder.add_recipient(recipient.clone(), Amount::from_sat(sats));
            }
            None => {
                builder.drain_wallet().drain_to(recipient.clone());
            }
        }
        builder.fee_rate(fee_rate);
        if let Some(height) = shape.lock_height {
            builder.nlocktime(LockTime::from_height(height).map_err(|err| PayError::Failed(err.to_string()))?);
        }
        if !shape.spend.is_empty() {
            builder
                .add_utxos(&shape.spend)
                .map_err(|err| PayError::Conflict(format!("A coin this payment must spend is gone: {err}")))?;
        }
        for outpoint in untrusted
            .into_iter()
            .chain(self.escrow.coins.outpoints())
            .chain(blocked)
        {
            builder.add_unspendable(outpoint);
        }
        let psbt = builder
            .finish()
            .map_err(|err| build_error(err, held_back))?;
        let foreign = psbt
            .unsigned_tx
            .output
            .iter()
            .filter(|out| {
                out.script_pubkey != recipient && !wallet.wallet.is_mine(out.script_pubkey.clone())
            })
            .count();
        if foreign != 0 || paid_to(&psbt, &recipient).is_none() {
            return Err(PayError::Failed(
                "a send must pay exactly one address".into(),
            ));
        }
        wallet
            .persist()
            .map_err(|err| PayError::Failed(err.to_string()))?;
        Ok((psbt, held_back))
    }

    /// `POST /wallet/send/quote`: the exact send the user is asked to confirm.
    pub(crate) fn quote_send(&self, body: &Value) -> Result<(String, SendTerms, u64), PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        let request = parse_quote(body, self.network)?;
        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let recipient = request.address.script_pubkey();
        let (psbt, held_back) = self.build_send(
            &request.address,
            request.amount,
            fee_rate,
            request.link_coins,
        )?;
        let fee = psbt_fee(&psbt)?;
        let terms = SendTerms {
            address: request.address.to_string(),
            amount_sats: paid_to(&psbt, &recipient).unwrap_or_default(),
            recipient,
            max: request.amount.is_none(),
            link_coins: request.link_coins,
            fee_sats: fee,
            fee_cap_sats: (fee * FEE_CAP_PERCENT).div_ceil(100),
            fee_rate,
        };
        Ok((
            self.lock.send_quotes.offer(terms.clone(), self.lock.now()),
            terms,
            held_back,
        ))
    }

    /// `POST /wallet/send`, after the passphrase: spend `quote` once.
    pub(crate) fn send_quoted(&self, quote: &str) -> Result<Value, PayError> {
        let terms = self
            .lock
            .send_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| PayError::Conflict(QUOTE_USED.into()))?;
        let address = address_for(&terms.address, self.network).map_err(PayError::Invalid)?;
        let _spending = self.spending.lock().unwrap_or_else(|err| err.into_inner());
        let amount = (!terms.max).then_some(terms.amount_sats);
        let (mut psbt, _) = self.build_send(&address, amount, terms.fee_rate, terms.link_coins)?;
        if paid_to(&psbt, &terms.recipient) != Some(terms.amount_sats) {
            return Err(PayError::Conflict(
                "The wallet's balance changed since the quote. Nothing was sent; review the send again.".into(),
            ));
        }
        let fee = psbt_fee(&psbt)?;
        if fee > terms.fee_cap_sats {
            return Err(PayError::Conflict(format!(
                "The network fee is now {fee} sats, above the {} sats you confirmed. Nothing was sent; review the send again.",
                terms.fee_cap_sats
            )));
        }
        let tx = {
            let wallet = self.buyer_wallet();
            let seed = self.spend_seed(&wallet)?;
            if !wallet
                .sign(&mut psbt, &seed)
                .map_err(|err| PayError::Failed(err.to_string()))?
            {
                return Err(PayError::Failed(
                    "the send could not be fully signed".into(),
                ));
            }
            psbt.extract_tx()
                .map_err(|err| PayError::Failed(err.to_string()))?
        };
        self.chain.broadcast(&tx).map_err(PayError::Chain)?;
        let txid = tx.compute_txid().to_string();
        {
            let mut wallet = self.buyer_wallet();
            wallet.wallet.apply_unconfirmed_txs([(tx, now_secs())]);
            if let Err(err) = wallet.persist() {
                eprintln!("ob-node: wallet state after {txid}: {err}");
            }
        }
        self.lock.touch();
        eprintln!("ob-node: sent {txid} from the node wallet");
        let mut value = terms.value(self.network.as_str());
        value["txid"] = json!(txid);
        value["feeSats"] = json!(fee);
        Ok(value)
    }
}

fn read(body: &[u8], origin: Option<&str>) -> Result<Value, Vec<u8>> {
    if body.len() > MAX_BODY {
        return Err(json_error(
            "400 Bad Request",
            "That request is too large.",
            origin,
        ));
    }
    serde_json::from_slice(body)
        .map_err(|_| json_error("400 Bad Request", "The request could not be read.", origin))
}

fn send_failed(err: PayError, origin: Option<&str>) -> Vec<u8> {
    match err {
        PayError::Failed(detail) => {
            eprintln!("ob-node: wallet send: {detail}");
            json_error(
                "500 Internal Server Error",
                "The send failed. Details are in the node log.",
                origin,
            )
        }
        err => pay_error(err, origin),
    }
}

pub(crate) fn quote_response(
    node: &Node,
    client: &Client,
    head: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    let parsed = match read(body, origin) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    match node.quote_send(&parsed) {
        Ok((quote, terms, held_back)) => {
            let mut value = terms.value(node.network.as_str());
            value["quote"] = json!(quote);
            value["heldBackSats"] = json!(held_back);
            json_body("200 OK", &value, origin)
        }
        Err(err) => send_failed(err, origin),
    }
}

pub(crate) fn send_response(
    node: &Node,
    client: &Client,
    head: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    confirmed_response(node, body, origin, |quote| node.send_quoted(quote))
}

/// The second step of every send, after the caller's `local_guard`: read
/// `{quote, passphrase, confirm: true}`, check the passphrase (a wrong one
/// leaves the quote open), then hand the quote to `send`, which uses it up.
pub(crate) fn confirmed_response(
    node: &Node,
    body: &[u8],
    origin: Option<&str>,
    send: impl FnOnce(&str) -> Result<Value, PayError>,
) -> Vec<u8> {
    let parsed = match read(body, origin) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let (quote, passphrase) = match confirmed_send(&parsed) {
        Ok(fields) => fields,
        Err(err) => return pay_error(err, origin),
    };
    if let Err(err) = node.lock.verify(passphrase) {
        return lock_error(err, origin);
    }
    match send(quote) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => send_failed(err, origin),
    }
}

/// The quote and passphrase of a send request that says `"confirm": true`.
fn confirmed_send(body: &Value) -> Result<(&str, &str), PayError> {
    let invalid = |message: &str| PayError::Invalid(message.into());
    let map = body
        .as_object()
        .ok_or_else(|| invalid("The request could not be read."))?;
    if map
        .keys()
        .any(|key| !["quote", "passphrase", "confirm"].contains(&key.as_str()))
    {
        return Err(invalid("That field is not part of a send."));
    }
    if map.get("confirm") != Some(&Value::Bool(true)) {
        return Err(invalid(
            "Review the send, then confirm it with \"confirm\": true.",
        ));
    }
    let quote = map
        .get("quote")
        .and_then(Value::as_str)
        .filter(|quote| !quote.is_empty());
    let quote = quote.ok_or_else(|| invalid("Every send needs a quote. Review the send first."))?;
    let passphrase = map
        .get("passphrase")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    let passphrase = passphrase.ok_or_else(|| invalid("Enter the wallet passphrase to send."))?;
    Ok((quote, passphrase))
}
