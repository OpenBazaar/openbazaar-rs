//! Watching this node's on-chain fee payments until they confirm.
//!
//! Once per block, a payment with no receipt yet that the chain source
//! lacks is broadcast again, up to the paid request's due height. It is
//! cleared, its coins handed back to the wallet, and the request made
//! payable again only on evidence that it cannot confirm: a confirmed
//! transaction in the wallet spends one of its inputs, or the chain source
//! still lacks it [`UNSEEN_BLOCKS`] blocks after first missing it and
//! [`RESENDS`] broadcasts (fewer once the due height passed, when it can
//! no longer count). A confirmed payment is never cleared. A payment made
//! after a cleared one spends one of its unspent coins, so at most one of
//! them confirms, and a cleared payment that confirms anyway becomes the
//! payment again.

use std::collections::HashSet;

use bitcoin::{OutPoint, Transaction};
use openbazaar_escrow::Party;

use super::{FilerFee, Payment, Unseen, MAX_KEPT};
use crate::buyer_orders::now_secs;
use crate::escrow_mail::{parse_id, EscrowId};
use crate::escrow_store::Role;
use crate::tx_watch::Seen;
use crate::Node;

const RESENDS: u32 = 3;
const UNSEEN_BLOCKS: u32 = 6;
const TICK_SECS: u64 = 5;

/// When the filer tick last ran, and at which tip.
#[derive(Default)]
pub(crate) struct FilerTick {
    at: u64,
    tip: u32,
}

impl FilerFee {
    /// Move the payment to the cleared ones; the request is payable again.
    pub(crate) fn clear_payment(&mut self) {
        if let Some(payment) = self.payment.take() {
            self.cleared.push(Payment { unseen: None, ..payment });
            if self.cleared.len() > MAX_KEPT {
                self.cleared.remove(0);
            }
        }
    }

    /// An unconfirmed payment to watch, or cleared ones that could still count.
    fn watched(&self, tip: u32) -> bool {
        let unconfirmed = self.payment.as_ref().is_some_and(|payment| payment.tx.is_some());
        let open = self
            .latest()
            .is_some_and(|(request, _)| tip <= request.due_height.saturating_add(UNSEEN_BLOCKS));
        self.receipt.is_none() && self.recused.is_none() && (unconfirmed || open && !self.cleared.is_empty())
    }

    fn due_of(&self, payment: &Payment) -> Option<u32> {
        self.requests.iter().find_map(|bytes| {
            let signed = openbazaar_protocol::SignedFeeRequest::decode(bytes).ok()?;
            (signed.hash().ok()? == payment.request).then_some(signed.terms.due_height)
        })
    }
}

fn record_of(name: &str) -> Option<(Party, EscrowId)> {
    let (id, role) = name.rsplit_once('-')?;
    let party = match role {
        "buyer" => Party::Buyer,
        "vendor" => Party::Vendor,
        _ => return None,
    };
    Some((party, parse_id(id)?))
}

impl Node {
    /// Watch fee payments once per block. Called from the mail loop.
    pub(crate) fn filer_fee_tick(&self) {
        {
            let mut tick = self.escrow.filer_tick.lock().unwrap_or_else(|err| err.into_inner());
            let now = now_secs();
            if now < tick.at.saturating_add(TICK_SECS) {
                return;
            }
            tick.at = now;
        }
        let Ok(tip) = self.chain.tip_height() else { return };
        {
            let mut tick = self.escrow.filer_tick.lock().unwrap_or_else(|err| err.into_inner());
            if tick.tip == tip {
                return;
            }
            tick.tip = tip;
        }
        self.filer_fee_steps(tip);
    }

    /// One pass of [`Self::filer_fee_tick`] at `tip`, unthrottled.
    pub(crate) fn filer_fee_steps(&self, tip: u32) {
        for (name, record) in self.escrow.store.list(Role::Filer) {
            let Some((party, id)) = record_of(&name) else { continue };
            if !FilerFee::from_record(&record).is_some_and(|fee| fee.watched(tip)) {
                continue;
            }
            if let Err(err) = self.watch_fee_payment(party, &id, tip) {
                eprintln!("ob-node: dispute fee payment {name}: {err}");
            }
        }
    }

    fn watch_fee_payment(&self, party: Party, id: &EscrowId, tip: u32) -> Result<(), String> {
        let lock = self.escrow.locks.get(id);
        let _held = lock.lock().unwrap_or_else(|err| err.into_inner());
        let mut fee = self.filer_fee(party, id)?;
        if !fee.watched(tip) {
            return Ok(());
        }
        let before = fee.clone();
        if let Some(mut payment) = fee.payment.clone() {
            let due = fee.due_of(&payment).unwrap_or(0);
            if let Some(tx) = payment.tx.clone() {
                let dropped = self.dropped(&mut payment, &tx, due, tip);
                fee.payment = Some(payment);
                if dropped {
                    self.evict(&tx);
                    fee.clear_payment();
                    eprintln!("ob-node: a dispute fee payment {} cannot confirm; it was cleared", tx.compute_txid());
                }
            }
        }
        if fee.payment.is_none() {
            let confirmed = fee
                .cleared
                .iter()
                .rposition(|cleared| cleared.tx.as_ref().is_some_and(|tx| matches!(self.tx_seen(tx), Seen::Confirmed(_))));
            if let Some(index) = confirmed {
                fee.payment = Some(fee.cleared.remove(index));
            }
        }
        if fee != before {
            self.save_filer_fee(party, id, &fee)?;
        }
        Ok(())
    }

    /// Whether an unconfirmed `tx` can no longer confirm, after sending it
    /// again while it still may and still counts.
    fn dropped(&self, payment: &mut Payment, tx: &Transaction, due: u32, tip: u32) -> bool {
        let seen = self.tx_seen(tx);
        if matches!(seen, Seen::Confirmed(_)) {
            payment.unseen = None;
            return false;
        }
        if self.spent_elsewhere(tx) {
            return true;
        }
        let counts = tip <= due;
        match seen {
            Seen::Pending | Seen::Confirmed(_) => {
                payment.unseen = None;
                false
            }
            Seen::Unknown => {
                if counts {
                    self.rebroadcast_unseen(tx);
                }
                false
            }
            Seen::Unseen => {
                let unseen = payment.unseen.get_or_insert(Unseen { since: tip, resent: 0 });
                if counts {
                    self.rebroadcast_unseen(tx);
                    unseen.resent += 1;
                }
                tip >= unseen.since.saturating_add(UNSEEN_BLOCKS) && (unseen.resent >= RESENDS || !counts)
            }
        }
    }

    /// Whether a confirmed transaction in the wallet spends an input of `tx`.
    fn spent_elsewhere(&self, tx: &Transaction) -> bool {
        let txid = tx.compute_txid();
        let inputs: HashSet<OutPoint> = tx.input.iter().map(|input| input.previous_output).collect();
        let wallet = self.buyer_wallet();
        let spent = wallet.wallet.transactions().any(|other| {
            other.tx_node.txid != txid
                && other.chain_position.confirmation_height_upper_bound().is_some()
                && other.tx_node.tx.input.iter().any(|input| inputs.contains(&input.previous_output))
        });
        spent
    }

    /// Drop `tx` from the wallet's view, so its coins can be spent again.
    fn evict(&self, tx: &Transaction) {
        let mut wallet = self.buyer_wallet();
        wallet.wallet.apply_evicted_txs([(tx.compute_txid(), now_secs())]);
        if let Err(err) = wallet.persist() {
            eprintln!("ob-node: wallet state after clearing {}: {err}", tx.compute_txid());
        }
    }

    /// A still-unspent coin of each cleared payment, for the next payment to spend.
    pub(crate) fn cleared_coins(&self, fee: &FilerFee) -> Vec<OutPoint> {
        let wallet = self.buyer_wallet();
        fee.cleared
            .iter()
            .filter_map(|cleared| cleared.tx.as_ref())
            .filter_map(|tx| {
                tx.input
                    .iter()
                    .map(|input| input.previous_output)
                    .find(|outpoint| wallet.wallet.get_utxo(*outpoint).is_some())
            })
            .collect()
    }
}
