//! Paying a filing fee back from the moderator's wallet.
//!
//! The operator can recuse from a paid case before any letter of it is
//! decided: this node signs a public decision (`OBDD` recused), which is
//! final, and pays the fee back on-chain. The refund goes to the refund
//! script in the payer's own v5 letter, and this node pays the network fee.
//!
//! | route | lock | body | answer |
//! |---|---|---|---|
//! | `POST /moderator/disputes/{id}/recuse/quote` | unlocked | none | the refund to confirm, `{quote, ...}` |
//! | `POST /moderator/disputes/{id}/recuse` | unlocked | `{quote, passphrase, confirm: true}` | `{txid, ...}` |
//!
//! A refund is stored with the fee before it is broadcast, and broadcast
//! again from the fee tick until the chain source has it, so a crash in
//! between pays once.

use bitcoin::{Address, FeeRate, ScriptBuf, Transaction};
use openbazaar_escrow::{Decision, Party};
use serde_json::{json, Value};

use super::case::party_name;
use super::fee::{EscrowFee, Refund};
use crate::buyer_orders::now_secs;
use crate::escrow_mail::{id_hex, parse_id, EscrowId, Kind, Mail};
use crate::http::{json_body, local_guard, Client};
use crate::lock_http::pay_error;
use crate::pay_order::{psbt_fee, PayError};
use crate::spend_tickets::{FEE_CAP_PERCENT, TICKET_TTL};
use crate::wallet_send::{confirmed_response, QUOTE_USED};
use crate::Node;

/// What one refund is allowed to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RefundTerms {
    pub escrow: EscrowId,
    pub filer: Party,
    pub script: ScriptBuf,
    pub amount_sats: u64,
    pub fee_rate: FeeRate,
    pub fee_sats: u64,
    pub fee_cap_sats: u64,
}

impl RefundTerms {
    fn value(&self, network: bitcoin::Network) -> Value {
        json!({
            "escrowId": id_hex(&self.escrow),
            "filer": party_name(self.filer),
            "amountSats": self.amount_sats,
            "address": Address::from_script(&self.script, network).ok().map(|address| address.to_string()),
            "feeSats": self.fee_sats,
            "feeCapSats": self.fee_cap_sats,
            "totalSats": self.amount_sats.saturating_add(self.fee_sats),
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

fn conflict(message: &str) -> PayError {
    PayError::Conflict(message.into())
}

/// Who a refund goes to, where, and how much.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    filer: Party,
    script: Vec<u8>,
    amount: u64,
}

impl Node {
    /// The recusal refund of escrow `id`, checked against the desk now.
    fn refund_target(&self, id: &EscrowId) -> Result<Target, PayError> {
        let desk = self.moderator_desk();
        let fee = desk.fees.get(id).ok_or(PayError::NotFound)?;
        let paid = fee
            .paid
            .ok_or_else(|| conflict("Nothing was paid for this case, so there is nothing to refund."))?;
        if fee.recusal.is_some() {
            return Err(conflict("This node already recused itself from the case."));
        }
        let decided = desk
            .cases
            .iter()
            .any(|((case_id, _), case)| case_id == id && (case.ruling.is_some() || case.decision.is_some()));
        if decided {
            return Err(conflict("A letter of this case is decided. A recusal must come before any decision."));
        }
        let script = desk
            .cases
            .get(&(*id, paid.filer as u8))
            .and_then(|case| case.refund_script.clone())
            .ok_or_else(|| conflict("The payer's letter named no refund script."))?;
        Ok(Target {
            filer: paid.filer,
            script,
            amount: paid.received_sats,
        })
    }

    fn quote_refund(&self, escrow: EscrowId, target: Target) -> Result<RefundTerms, PayError> {
        let script = ScriptBuf::from_bytes(target.script);
        if target.amount < script.minimal_non_dust().to_sat() {
            return Err(conflict("That payment is too small to send back on-chain."));
        }
        let address = Address::from_script(&script, self.network.bitcoin())
            .map_err(|_| conflict("The refund script has no address on this network."))?;
        self.sync_buyer_wallet().map_err(PayError::Chain)?;
        let fee_rate = self.pay_fee_rate()?;
        let (psbt, _) = self.build_send(&address, Some(target.amount), fee_rate, false)?;
        let fee_sats = psbt_fee(&psbt)?;
        Ok(RefundTerms {
            escrow,
            filer: target.filer,
            script,
            amount_sats: target.amount,
            fee_rate,
            fee_sats,
            fee_cap_sats: (fee_sats * FEE_CAP_PERCENT).div_ceil(100),
        })
    }

    /// `POST /moderator/disputes/{id}/recuse/quote`.
    pub(crate) fn quote_recusal(&self, id: &str) -> Result<Value, PayError> {
        self.lock.spend_key()?;
        self.lock.touch();
        let escrow = parse_id(id).ok_or(PayError::NotFound)?;
        let terms = self.quote_refund(escrow, self.refund_target(&escrow)?)?;
        let mut value = terms.value(self.network.bitcoin());
        value["quote"] = json!(self.lock.refund_quotes.offer(terms, self.lock.now()));
        Ok(value)
    }

    /// `POST /moderator/disputes/{id}/recuse`, after the passphrase: sign
    /// the recusal, pay the fee back, and tell every filer.
    pub(crate) fn recuse_dispute(&self, id: &str, quote: &str) -> Result<Value, PayError> {
        let terms = self.take_refund_quote(id, quote)?;
        let lock = self.escrow.locks.get(&terms.escrow);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let now = self.refund_target(&terms.escrow)?;
        if now != (Target { filer: terms.filer, script: terms.script.to_bytes(), amount: terms.amount_sats }) {
            return Err(conflict("The case changed since the quote. Nothing was paid; review the refund again."));
        }
        let (record, case_id) = {
            let desk = self.moderator_desk();
            let case = desk
                .cases
                .get(&(terms.escrow, terms.filer as u8))
                .ok_or(PayError::NotFound)?;
            let fee = desk.fees.get(&terms.escrow).ok_or(PayError::NotFound)?;
            (case.record.clone(), fee.case_id)
        };
        let decision = self
            .case_keys(record.as_deref())
            .and_then(|keys| {
                keys.sign_dispute_decision(case_id, Decision::Recused, None)
                    .map_err(|err| err.to_string())
            })
            .map_err(PayError::Failed)?;
        let tx = self.pay_refund(&terms, |fee| fee.recusal = Some(decision.clone()))?;
        let txid = tx.compute_txid();
        let filers: Vec<(Party, String, tormail::StoreKey)> = self
            .moderator_desk()
            .cases
            .iter()
            .filter(|((case_id, _), _)| *case_id == terms.escrow)
            .map(|((_, branch), case)| (super::case::party_of(*branch), case.reply_url.clone(), case.reply_mail))
            .collect();
        let mut refused = None;
        for (party, url, to) in filers {
            let mut fields = vec![decision.clone()];
            if party == terms.filer {
                fields.push(bitcoin::hashes::Hash::to_byte_array(txid).to_vec());
            }
            if let Err(err) = self.send_mail(&url, &to, &Mail::new(Kind::Recused, terms.escrow, fields)) {
                refused.get_or_insert(err);
            }
        }
        if let Some(err) = refused {
            return Err(PayError::Chain(format!(
                "The fee was paid back in {txid}, but a party's node refused the recusal: {err}"
            )));
        }
        let mut value = terms.value(self.network.bitcoin());
        value["txid"] = json!(txid.to_string());
        value["recused"] = json!(true);
        Ok(value)
    }

    fn take_refund_quote(&self, id: &str, quote: &str) -> Result<RefundTerms, PayError> {
        let terms = self
            .lock
            .refund_quotes
            .take_quote(quote, self.lock.now())
            .map_err(|_| conflict(QUOTE_USED))?;
        if parse_id(id) != Some(terms.escrow) {
            return Err(conflict("That quote is for another refund. Nothing was paid."));
        }
        Ok(terms)
    }

    /// Sign the refund, store it with `record` applied to the escrow's fee,
    /// hand it to the wallet, then broadcast it. The escrow's lock is held.
    fn pay_refund(&self, terms: &RefundTerms, record: impl FnOnce(&mut EscrowFee)) -> Result<Transaction, PayError> {
        let _spending = self.spending.lock().unwrap_or_else(|err| err.into_inner());
        let (tx, _) = self.signed_send(&terms.script, terms.amount_sats, terms.fee_rate, terms.fee_cap_sats)?;
        let mut fee = self
            .moderator_desk()
            .fees
            .get(&terms.escrow)
            .cloned()
            .ok_or(PayError::NotFound)?;
        record(&mut fee);
        fee.refunds.push(Refund {
            filer: terms.filer,
            amount: terms.amount_sats,
            tx: tx.clone(),
        });
        self.save_fee(&terms.escrow, &fee).map_err(PayError::Failed)?;
        self.moderator_desk().fees.insert(terms.escrow, fee);
        {
            let mut wallet = self.buyer_wallet();
            wallet.wallet.apply_unconfirmed_txs([(tx.clone(), now_secs())]);
            if let Err(err) = wallet.persist() {
                eprintln!("ob-node: wallet state after refund {}: {err}", tx.compute_txid());
            }
        }
        if let Err(err) = self.chain.broadcast(&tx) {
            eprintln!("ob-node: refund {} is stored and will be sent again: {err}", tx.compute_txid());
        }
        Ok(tx)
    }

    /// Broadcast again every stored refund that is not confirmed and that
    /// the chain source does not have.
    pub(super) fn rebroadcast_refunds(&self, fee: &EscrowFee) {
        for refund in &fee.refunds {
            self.rebroadcast_unseen(&refund.tx);
        }
    }
}

pub(crate) fn recuse_quote_response(node: &Node, client: &Client, head: &str, id: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    match node.quote_recusal(id) {
        Ok(value) => json_body("200 OK", &value, origin),
        Err(err) => pay_error(err, origin),
    }
}

pub(crate) fn recuse_response(
    node: &Node,
    client: &Client,
    head: &str,
    id: &str,
    body: &[u8],
    origin: Option<&str>,
) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    confirmed_response(node, body, origin, |quote| node.recuse_dispute(id, quote))
}
