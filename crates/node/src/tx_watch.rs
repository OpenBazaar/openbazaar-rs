//! Where a transaction this node signed stands, and sending it again.
//!
//! Sending again is idempotent: a transaction the chain source already
//! holds, or one confirmed in the node wallet, is left alone.

use bitcoin::Transaction;

use crate::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Seen {
    /// In a block, by the node wallet or the chain source: `Some(height)`
    /// when the height is known.
    Confirmed(Option<u32>),
    /// The chain source holds it unconfirmed.
    Pending,
    /// Neither the node wallet nor the chain source has it.
    Unseen,
    /// The chain source could not be asked.
    Unknown,
}

impl Node {
    pub(crate) fn tx_seen(&self, tx: &Transaction) -> Seen {
        let txid = tx.compute_txid();
        let in_wallet = self
            .buyer_wallet()
            .wallet
            .get_tx(txid)
            .and_then(|tx| tx.chain_position.confirmation_height_upper_bound());
        if let Some(height) = in_wallet {
            return Seen::Confirmed(Some(height));
        }
        match self.chain.transaction(&txid) {
            Ok(Some((_, Some(height)))) => Seen::Confirmed(Some(height)),
            Ok(Some((_, None))) => Seen::Pending,
            Ok(None) => Seen::Unseen,
            Err(_) => Seen::Unknown,
        }
    }

    /// Broadcast `tx` again unless it is confirmed or the chain source has
    /// it, and say where it stood.
    pub(crate) fn rebroadcast_unseen(&self, tx: &Transaction) -> Seen {
        let seen = self.tx_seen(tx);
        if matches!(seen, Seen::Unseen | Seen::Unknown) {
            if let Err(err) = self.chain.broadcast(tx) {
                eprintln!("ob-node: sending {} again: {err}", tx.compute_txid());
            }
        }
        seen
    }
}
