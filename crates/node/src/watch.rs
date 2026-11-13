//! Order status from the chain.
//!
//! The watcher runs on its own thread, never on the HTTP accept loop. Each
//! round it reads the tip, asks the chain source about every open order's
//! address, and writes any status change into the sealed order record. With
//! no open order and no open sale the tip is not read: nothing would count
//! confirmations with it, and the chain source may be a public server.
//!
//! Direct: Awaiting payment -> Payment seen (in the mempool, or fewer than N
//! confirmations) -> Paid (N confirmations). Less than the amount due stays
//! Awaiting payment, with `receivedSats` showing what arrived.
//!
//! Escrow: Awaiting vendor terms (set by checkout, left alone here) ->
//! Awaiting funding -> Funding seen -> Funded. Funded needs one output
//! of exactly `fundingSats` at the outpoint the presigned refund spends. A
//! funding the node holds no refund for is reported as "Funded without a refund".

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bitcoin::OutPoint;
use serde_json::Value;

use crate::buyer_orders::now_secs;
use crate::chain::AddressTx;
use crate::network::address_for;
use crate::Node;

pub const AWAITING_PAYMENT: &str = "Awaiting payment";
pub const PAYMENT_SEEN: &str = "Payment seen";
pub const PAID: &str = "Paid";
pub const AWAITING_FUNDING: &str = "Awaiting funding";
pub const FUNDING_SEEN: &str = "Funding seen";
pub const FUNDED: &str = "Funded";
pub const FUNDED_WITHOUT_REFUND: &str = "Funded without a refund";
/// Escrow statuses set by the escrow rounds. The watcher leaves these alone.
pub const AWAITING_TERMS: &str = "Awaiting vendor terms";
pub const RELEASED: &str = "Released";
pub const DISPUTED: &str = "Disputed";
pub const SETTLED: &str = "Settled by the moderator";
/// Orders to a store on another node ([`crate::remote_orders`]), set from
/// the seller's replies. The watcher leaves these alone.
pub const AWAITING_SELLER: &str = "Awaiting seller";
pub const DECLINED: &str = "Declined by the seller";
pub const SHIPPED: &str = "Shipped";
pub const COMPLETED: &str = "Completed";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub status: &'static str,
    pub received_sats: u64,
    pub confirmations: u32,
    pub txids: Vec<String>,
}

fn confirmations(tip: u32, height: Option<u32>) -> u32 {
    match height {
        Some(height) if height <= tip => tip - height + 1,
        _ => 0,
    }
}

fn txids(txs: &[AddressTx]) -> Vec<String> {
    txs.iter().map(|tx| tx.txid.to_string()).collect()
}

/// Direct pay on either rail: nothing due stays awaiting; the full amount
/// seen but not confirmed is payment seen; the full amount confirmed is paid.
pub(crate) fn direct_progress(due: u64, received: u64, confirmed: u64) -> &'static str {
    if due > 0 && confirmed >= due {
        PAID
    } else if due > 0 && received >= due {
        PAYMENT_SEEN
    } else {
        AWAITING_PAYMENT
    }
}

pub(crate) fn direct_status(due: u64, txs: &[AddressTx], tip: u32, needed: u32) -> Observation {
    let received: u64 = txs.iter().map(AddressTx::value).sum();
    let settled: u64 = txs
        .iter()
        .filter(|tx| confirmations(tip, tx.height) >= needed)
        .map(AddressTx::value)
        .sum();
    let status = direct_progress(due, received, settled);
    Observation {
        status,
        received_sats: received,
        confirmations: txs
            .iter()
            .map(|tx| confirmations(tip, tx.height))
            .min()
            .unwrap_or(0),
        txids: txids(txs),
    }
}

pub(crate) fn escrow_status(
    funding_sats: u64,
    refund_spends: Option<OutPoint>,
    txs: &[AddressTx],
    tip: u32,
    needed: u32,
) -> Observation {
    let received: u64 = txs.iter().map(AddressTx::value).sum();
    let exact = |tx: &AddressTx| {
        tx.outputs
            .iter()
            .filter(|(_, sats)| *sats == funding_sats)
            .map(|(vout, _)| OutPoint { txid: tx.txid, vout: *vout })
            .collect::<Vec<_>>()
    };
    let covered = txs
        .iter()
        .find(|tx| refund_spends.is_some_and(|outpoint| exact(tx).contains(&outpoint)));
    let (status, depth) = match covered {
        Some(tx) => {
            let depth = confirmations(tip, tx.height);
            (if depth >= needed { FUNDED } else { FUNDING_SEEN }, depth)
        }
        None => match txs.iter().find(|tx| !exact(tx).is_empty()) {
            Some(tx) => (FUNDED_WITHOUT_REFUND, confirmations(tip, tx.height)),
            None => (AWAITING_FUNDING, 0),
        },
    };
    Observation {
        status,
        received_sats: received,
        confirmations: depth,
        txids: txids(txs),
    }
}

/// One order the watcher follows this round.
struct Target {
    id: String,
    escrow: bool,
    address: String,
    due: u64,
    refund_spends: Option<OutPoint>,
}

impl Node {
    /// One watcher round. Returns how many orders changed.
    pub fn watch_once(&self) -> Result<usize, String> {
        if let Err(err) = self.sync_buyer_wallet() {
            eprintln!("ob-node: wallet sync: {err}");
        }
        let mut changed = self.expire_escrow_opens(crate::escrow_outbox::now_secs());
        let targets = self.watch_targets();
        if targets.is_empty() && !self.sale_awaits_payment() {
            return Ok(changed + self.watch_stock() + self.publish_reviews(crate::escrow_outbox::now_secs()));
        }
        let tip = self.chain.tip_height()?;
        for target in targets {
            let Ok(address) = address_for(&target.address, self.network) else {
                continue;
            };
            let txs = match self.chain.address_txs(&address) {
                Ok(txs) => txs,
                Err(err) => {
                    eprintln!("ob-node: chain query for order {}: {err}", target.id);
                    continue;
                }
            };
            let seen = if target.escrow {
                escrow_status(target.due, target.refund_spends, &txs, tip, self.settings.confirmations)
            } else {
                direct_status(target.due, &txs, tip, self.settings.confirmations)
            };
            let mut orders = self.orders();
            let result = orders.update(&target.id, |order| {
                if order.status == seen.status
                    && order.received_sats == seen.received_sats
                    && order.confirmations == seen.confirmations
                    && order.txids == seen.txids
                {
                    return;
                }
                order.status = seen.status.to_string();
                order.received_sats = seen.received_sats;
                order.confirmations = seen.confirmations;
                order.txids = seen.txids.clone();
                changed += 1;
            });
            if let Err(err) = result {
                eprintln!("ob-node: order {}: {err}", target.id);
            }
        }
        changed += self.watch_sales(tip);
        changed += self.watch_tracking();
        changed += self.watch_stock();
        changed += self.publish_reviews(crate::escrow_outbox::now_secs());
        Ok(changed)
    }

    /// A sale [`Node::watch_sales`] would look up on the chain.
    fn sale_awaits_payment(&self) -> bool {
        self.sales().list().any(|sale| sale["status"] == AWAITING_PAYMENT || sale["status"] == PAYMENT_SEEN)
    }

    fn watch_targets(&self) -> Vec<Target> {
        let orders = self.orders();
        orders
            .on_network()
            .filter(|order| {
                ![PAID, FUNDED, AWAITING_TERMS, RELEASED, DISPUTED, SETTLED, AWAITING_SELLER, DECLINED, SHIPPED, COMPLETED]
                    .contains(&order.status.as_str())
            })
            .filter_map(|order| {
                if order.settlement == "escrow" {
                    let escrow = order.escrow.as_ref()?;
                    Some(Target {
                        id: order.id.clone(),
                        escrow: true,
                        address: escrow.get("address")?.as_str()?.to_string(),
                        due: escrow.get("fundingSats")?.as_u64()?,
                        refund_spends: escrow
                            .get("outpoint")
                            .and_then(Value::as_str)
                            .and_then(|text| text.parse().ok()),
                    })
                } else {
                    let payment = order.payment.as_ref()?;
                    Some(Target {
                        id: order.id.clone(),
                        escrow: false,
                        address: payment.get("address")?.as_str()?.to_string(),
                        due: order.total_sats,
                        refund_spends: None,
                    })
                }
            })
            .collect()
    }

    /// Bring the node wallet up to date. The wallet lock is not held during the query.
    pub(crate) fn sync_buyer_wallet(&self) -> Result<(), String> {
        let request = self.buyer_wallet().update_request();
        let result = self.chain.wallet_update(request);
        let mut wallet = self.buyer_wallet();
        match result {
            Ok(update) => wallet.apply(update, now_secs()).map_err(|err| err.to_string()),
            Err(err) => {
                wallet.sync_error = Some(err.clone());
                Err(err)
            }
        }
    }

    /// Run [`Node::watch_once`] every `every` until `stop` is set.
    pub(crate) fn run_watcher(&self, every: Duration, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match self.watch_once() {
                Ok(0) => {}
                Ok(changed) => eprintln!("ob-node: {changed} order status change(s)"),
                Err(err) => eprintln!("ob-node: watcher: {err}"),
            }
            let mut slept = Duration::ZERO;
            while slept < every && !stop.load(Ordering::Relaxed) {
                let step = Duration::from_millis(250).min(every - slept);
                std::thread::sleep(step);
                slept += step;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_payment_waits_until_the_full_amount_is_confirmed() {
        assert_eq!(direct_progress(10, 0, 0), AWAITING_PAYMENT);
        assert_eq!(direct_progress(10, 4, 4), AWAITING_PAYMENT);
        assert_eq!(direct_progress(10, 10, 0), PAYMENT_SEEN);
        assert_eq!(direct_progress(10, 10, 9), PAYMENT_SEEN);
        assert_eq!(direct_progress(10, 12, 12), PAID);
        assert_eq!(direct_progress(0, 0, 0), AWAITING_PAYMENT);
    }
}
