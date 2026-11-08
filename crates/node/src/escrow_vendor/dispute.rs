//! Vendor-initiated disputes: `POST /escrow/sales/{id}/dispute` sends a
//! letter for the vendor-wins branch, and the moderator's ruling finishes
//! that outcome. Also the vendor's outcomes a fee bump can spend from.

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::Transaction;
use openbazaar_escrow::ModeratorRuling;
use serde_json::Value;

use super::failed;
use crate::escrow_fixture::moderator_mail_key;
use crate::escrow_mail::{parse_id, EscrowId, Kind, Mail};
use crate::instructions::ESCROW_MAINNET_REFUSAL;
use crate::network::NetworkMode;
use crate::pay_order::PayError;
use crate::Node;

const MAX_MEMO: usize = 1_000;

fn memo_of(body: &Value) -> Result<&str, PayError> {
    let memo = match body.as_object() {
        Some(map) if map.keys().all(|key| key == "memo") => {
            map.get("memo").and_then(Value::as_str).unwrap_or("")
        }
        _ => return Err(PayError::Invalid("Send a JSON object with a memo.".into())),
    };
    if memo.trim().is_empty() || memo.len() > MAX_MEMO {
        return Err(PayError::Invalid(format!(
            "Describe the problem in 1 to {MAX_MEMO} characters."
        )));
    }
    Ok(memo)
}

impl Node {
    /// Ask the moderator to rule for the vendor on a funded sale. Sending
    /// again while disputed sends a fresh letter.
    pub fn dispute_sale(&self, id: &str, body: &Value) -> Result<Value, PayError> {
        if self.network != NetworkMode::Testnet4 {
            return Err(PayError::Invalid(ESCROW_MAINNET_REFUSAL.into()));
        }
        let memo = memo_of(body)?;
        self.lock.spend_key()?;
        let escrow_id: EscrowId = parse_id(id).ok_or(PayError::NotFound)?;
        let moderator = self.escrow.config.moderator_url.clone().ok_or_else(|| {
            PayError::Invalid("This node has no moderator to send a dispute to.".into())
        })?;
        let reply = self.escrow_reply_url(&escrow_id).map_err(PayError::Invalid)?.to_string();
        let lock = self.escrow.locks.get(&escrow_id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let (seller, stage) = {
            let desk = self.vendor_desk();
            let sale = desk.sales.get(&escrow_id).ok_or(PayError::NotFound)?;
            (sale.seller.clone(), sale.stage.clone())
        };
        if stage != "funded" && stage != "disputed" {
            return Err(PayError::Conflict(format!("This sale is {stage}.")));
        }
        let mail_key = self
            .seller_mail_secret(&seller)
            .map(|secret| secret.public_key())
            .ok_or_else(|| PayError::Invalid("That seller has no mailbox key.".into()))?;
        let letter = self
            .with_vendor_escrow(
                &escrow_id,
                |live| live.escrow.dispute_letter(memo.as_bytes()).map_err(failed),
                |sale, _| sale.stage = "disputed".into(),
            )
            .map_err(PayError::Failed)?;
        let mail = Mail::new(
            Kind::VendorDispute,
            escrow_id,
            vec![letter, mail_key.as_bytes().to_vec(), reply.into_bytes()],
        );
        if let Err(err) = self.send_mail(&moderator, &moderator_mail_key(), &mail) {
            if stage == "funded" {
                let _ = self.update_sale(&escrow_id, |sale| sale.stage = "funded".into());
            }
            return Err(PayError::Chain(format!(
                "The moderator's node refused the dispute: {err}"
            )));
        }
        Ok(self.sales_value())
    }

    /// The vendor's presigned spend to bump, and the escrow's funding value.
    /// The claim is entered (its phase stored) the first time it is asked for.
    pub(crate) fn vendor_bump_parent(&self, id: &EscrowId, spend: &str) -> Result<Option<(Transaction, u64)>, String> {
        let Some((stage, funding, claim, outcome)) = self.vendor_desk().sales.get(id).map(|sale| {
            (sale.stage.clone(), sale.terms.funding_value.to_sat(), sale.claim.clone(), sale.outcome_tx.clone())
        }) else {
            return Ok(None);
        };
        let tx = match (spend, stage.as_str()) {
            ("settle", "settled") | ("claim", "claimed") => {
                deserialize_hex(&outcome.ok_or("the outcome is not stored")?).map_err(failed)?
            }
            ("claim", "funded" | "disputed") => {
                claim.ok_or("this sale holds no claim")?;
                let next = self.chain.tip_height()? + 1;
                self.with_vendor_escrow(
                    id,
                    |live| live.escrow.broadcast_claim(next).map_err(failed),
                    |sale, tx| {
                        sale.stage = "claimed".into();
                        sale.outcome_tx = Some(serialize_hex(tx));
                    },
                )?
            }
            _ => return Err(format!("this sale is {stage}; there is no {spend} to bump")),
        };
        Ok(Some((tx, funding)))
    }

    /// Finish the vendor-wins outcome with the moderator's ruling and broadcast it.
    pub(super) fn vendor_ruling(&self, seller: &str, mail: &Mail) -> Result<(), String> {
        match self.sale_of(seller, &mail.escrow, |sale| sale.stage.clone())?.as_str() {
            "disputed" => {}
            "settled" => return self.broadcast_outcome(&mail.escrow),
            other => return Err(format!("that sale is {other}")),
        }
        let ruling = ModeratorRuling::from_bytes(&mail.fields(1)?[0]).map_err(failed)?;
        self.with_vendor_escrow(
            &mail.escrow,
            |live| live.escrow.settle_dispute(&ruling).map_err(failed),
            |sale, tx| {
                sale.stage = "settled".into();
                sale.outcome_tx = Some(serialize_hex(tx));
                sale.settle_txid = Some(tx.compute_txid());
            },
        )?;
        self.broadcast_outcome(&mail.escrow)
    }
}
