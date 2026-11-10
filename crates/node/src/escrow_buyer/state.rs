//! The buyer's escrow on disk: snapshots, restore, and aborting an open.

use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::Transaction;
use openbazaar_escrow::{BuyerEscrow, ModeratorRuling, SignedRefund};
use serde_json::{json, Value};

use super::{escrow_id, failed, key_index, text};
use crate::buyer_orders::BuyerOrder;
use crate::escrow_mail::{hex, unhex, EscrowId};
use crate::escrow_store::{self, Role};
use crate::Node;

impl Node {
    pub(super) fn save_buyer_snapshot(&self, id: &EscrowId, escrow: &BuyerEscrow) -> Result<(), String> {
        self.escrow
            .store
            .save(Role::Buyer, &escrow_store::name(id), &json!({ "snapshot": hex(&escrow.snapshot()) }))
    }

    fn restore_buyer(&self, order: &BuyerOrder, id: &EscrowId) -> Result<BuyerEscrow, String> {
        let record = self
            .escrow
            .store
            .load(Role::Buyer, &escrow_store::name(id))?
            .ok_or("this node holds no signed state for that escrow")?;
        let bytes = unhex(record["snapshot"].as_str().ok_or("the escrow record has no snapshot")?)?;
        let escrow = order.escrow.as_ref().ok_or("that order has no escrow")?;
        let secret = {
            let wallet = self.buyer_wallet();
            self.escrow_secret(&wallet, key_index(escrow)?)?.secret_bytes()
        };
        let ctx = self.escrow_context(self.order_moderator_record(escrow)?)?;
        BuyerEscrow::restore(&bytes, &secret, &ctx).map_err(failed)
    }

    /// Run `step` on the live escrow, restoring it from its snapshot first
    /// if this process does not hold it (that needs the spend tier). A new
    /// snapshot is stored before this returns whenever the phase changed,
    /// so the caller may send what `step` produced.
    pub(super) fn with_buyer_escrow<T>(
        &self,
        order: &BuyerOrder,
        id: &EscrowId,
        step: impl FnOnce(&mut BuyerEscrow) -> Result<T, String>,
    ) -> Result<T, String> {
        if !self.buyer_desk().escrows.contains_key(id) {
            let restored = self.restore_buyer(order, id)?;
            self.buyer_desk().escrows.entry(*id).or_insert(restored);
        }
        let mut desk = self.buyer_desk();
        let escrow = desk.escrows.get_mut(id).ok_or("the escrow is gone")?;
        let before = escrow.phase();
        let out = step(escrow)?;
        if escrow.phase() != before {
            if let Err(err) = self.save_buyer_snapshot(id, escrow) {
                // Memory is ahead of the disk; drop it so the next step restores.
                desk.escrows.remove(id);
                return Err(err);
            }
        }
        Ok(out)
    }

    /// Finish the buyer-wins outcome. The outcome goes into the order record
    /// before the snapshot moves to `Disputed`.
    pub(super) fn settle_buyer(&self, order: &BuyerOrder, id: &EscrowId, ruling: &ModeratorRuling) -> Result<Transaction, String> {
        if !self.buyer_desk().escrows.contains_key(id) {
            let restored = self.restore_buyer(order, id)?;
            self.buyer_desk().escrows.entry(*id).or_insert(restored);
        }
        let tx = {
            let mut desk = self.buyer_desk();
            let escrow = desk.escrows.get_mut(id).ok_or("the escrow is gone")?;
            match escrow.settle_dispute(ruling) {
                Ok(tx) => tx,
                Err(err) => {
                    desk.escrows.remove(id);
                    return Err(err.to_string());
                }
            }
        };
        self.update_escrow(&order.id, None, json!({ "settleTx": serialize_hex(&tx) }))?;
        let desk = self.buyer_desk();
        let escrow = desk.escrows.get(id).ok_or("the escrow is gone")?;
        self.save_buyer_snapshot(id, escrow)?;
        Ok(tx)
    }

    /// The buyer's presigned spend to bump, and the escrow's funding value.
    /// The refund is entered (its phase stored) the first time it is asked for.
    pub(crate) fn buyer_bump_parent(&self, id: &EscrowId, spend: &str) -> Result<Option<(Transaction, u64)>, String> {
        let Ok(order) = self.escrow_order(id) else {
            return Ok(None);
        };
        let escrow = order.escrow.clone().ok_or("that order has no escrow")?;
        let funding = super::number(&escrow, "fundingSats")?;
        let stage = text(&escrow, "stage")?;
        let tx = match (spend, stage) {
            ("settle", "settled") => deserialize_hex(text(&escrow, "settleTx")?).map_err(failed)?,
            ("refund", "refunded") => {
                SignedRefund::from_bytes(&unhex(text(&escrow, "refund")?)?).map_err(failed)?.transaction().clone()
            }
            ("refund", "signed" | "funded" | "disputed") => {
                let next = self.chain.tip_height()? + 1;
                let tx = self.with_buyer_escrow(&order, id, |escrow| escrow.broadcast_refund(next).map_err(failed))?;
                self.update_escrow(&order.id, None, json!({ "stage": "refunded", "refundTxid": tx.compute_txid().to_string() }))?;
                tx
            }
            _ => return Err(format!("this escrow is {stage}; there is no {spend} to bump")),
        };
        Ok(Some((tx, funding)))
    }

    /// Give up on an open: forget the round 1 session and free its coins.
    pub(super) fn abort_open(&self, order_id: &str, id: &EscrowId, reason: &str) {
        self.buyer_desk().waiting.remove(id);
        if let Err(err) = self.escrow.coins.release(order_id) {
            eprintln!("ob-node: coin locks for {order_id}: {err}");
        }
        eprintln!("ob-node: escrow open for {order_id} aborted: {reason}");
    }

    /// Abort every open whose coins were locked past their time without a
    /// reply, and put the order back to "offered" so it can be paid again.
    /// Returns how many opens were aborted.
    pub(crate) fn expire_escrow_opens(&self, now: u64) -> usize {
        let mut aborted = 0;
        for order_id in self.escrow.coins.expired(now) {
            let order = self.orders().get(&order_id).cloned();
            let Some((order, escrow)) = order.and_then(|order| order.escrow.clone().map(|escrow| (order, escrow))) else {
                let _ = self.escrow.coins.release(&order_id);
                continue;
            };
            let Ok(id) = escrow_id(&escrow) else {
                continue;
            };
            let lock = self.escrow.locks.get(&id);
            let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
            let stage = self
                .orders()
                .get(&order_id)
                .and_then(|order| order.escrow.as_ref().and_then(|escrow| text(escrow, "stage").ok().map(str::to_string)));
            if stage.as_deref() != Some("opened") {
                let _ = self.escrow.coins.release(&order.id);
                continue;
            }
            self.abort_open(&order.id, &id, "the vendor did not answer in time");
            let reset = json!({
                "stage": "offered",
                "outpoint": Value::Null,
                "psbt": Value::Null,
                "error": "The vendor did not answer in time. Pay again to retry.",
            });
            if let Err(err) = self.update_escrow(&order.id, None, reset) {
                eprintln!("ob-node: order {}: {err}", order.id);
            }
            aborted += 1;
        }
        aborted
    }
}
