//! The node's own store on Zcash.
//!
//! Its wallet is the node wallet's phrase (`wallets/node`, ZIP 32 account 0),
//! kept like a demo store's: the viewing key in `zec-watch.obv` in the receive
//! tier, so a locked node scans and can hand out addresses. The choice is
//! bit 2 of the record other nodes follow, so changing it needs the store key
//! (an unlocked wallet), and `store-record.obv` keeps it for the next boot.
//! A buyer on another node pays it through a sale (see `sale.rs`).
//!
//! [`Node::own_zec_ready`] is the one check of whether the store can take a
//! Zcash order right now. Switching Zcash on and every sale both ask it.

use openbazaar_protocol::{ContentRating, Store};
use serde_json::{json, Value};

use super::AcceptError;
use crate::network::NetworkMode;
use crate::own_store::Rails;
use crate::Node;

pub(crate) const TESTNET4_ONLY: &str = "Your own store takes Zcash on testnet4 only for now. Mainnet follows once the node checks its Zcash indexer.";
pub(crate) const NEEDS_INDEXER: &str = "Zcash is off on this node. Start it with --zec-indexer.";

/// Whether a store on a node may take Zcash on `network`. Testnet4 only
/// until the indexer checks are in; return `true` to open mainnet.
pub(crate) fn own_store_allowed(network: NetworkMode) -> bool {
    network == NetworkMode::Testnet4
}

/// Why the own store cannot take a Zcash order right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NotReady {
    Network,
    Off,
    /// No viewing key yet (a wallet from before Zcash, never unlocked since), or the first sync is not done.
    NotSynced,
    NoPrice,
}

impl NotReady {
    /// For the vendor: what to do about it.
    pub(crate) fn vendor(self) -> &'static str {
        match self {
            NotReady::Network => TESTNET4_ONLY,
            NotReady::Off => NEEDS_INDEXER,
            NotReady::NotSynced => "This node has not read the Zcash chain yet. Unlock it if it is locked, then try again in a few minutes.",
            NotReady::NoPrice => "This node has no ZEC price right now, so it cannot price a Zcash order. Try again in a few minutes.",
        }
    }

    /// For a buyer on another node, as the reason an order is declined.
    pub(super) fn buyer(self) -> &'static str {
        match self {
            NotReady::Network => super::sale::NOT_TAKEN,
            NotReady::Off => "This store's node has Zcash switched off right now. Pay in bitcoin, or ask the seller to switch it back on.",
            NotReady::NotSynced => "This store's node is still reading the Zcash chain. Try again in a few minutes, or pay in bitcoin.",
            NotReady::NoPrice => "This store's node has no ZEC price right now. Try again in a few minutes, or pay in bitcoin.",
        }
    }
}

impl Node {
    /// Whether the own store can take a Zcash order right now: the network allows it, an indexer is set,
    /// the wallet has read the chain (an address handed out before the first sync could miss its payment),
    /// and there is a price, fresh or fixed.
    pub(crate) fn own_zec_ready(&self) -> Result<(), NotReady> {
        if !own_store_allowed(self.network) {
            return Err(NotReady::Network);
        }
        if self.zec.chain().is_none() {
            return Err(NotReady::Off);
        }
        if self.zec.own().wallet.as_ref().and_then(|wallet| wallet.scanned()).is_none() {
            return Err(NotReady::NotSynced);
        }
        self.sats_per_zec().map(|_| ()).ok_or(NotReady::NoPrice)
    }

    /// Opt the own store in or out by signing its record again. Turning it off is never refused for readiness.
    pub(super) fn set_own_accepts_zec(&self, store: &Store, accept: bool) -> Result<(), AcceptError> {
        if accept {
            self.own_zec_ready().map_err(AcceptError::NotReady)?;
        }
        let secret = self.store_secret(&store.id).ok_or(AcceptError::Locked)?;
        let rails = Rails { zec: accept, ..Rails::of(Some(store)) };
        self.publish_own(store.rating.unwrap_or(ContentRating::General), rails, &secret).map_err(|err| AcceptError::Failed(err.to_string()))
    }

    /// The own store's row in `GET /zec/stores`, once it has a record. `problem` is why a store with Zcash on would decline a Zcash order now.
    pub(super) fn own_zec_value(&self) -> Option<Value> {
        let store = self.own_record()?;
        let accepts = store.details.is_some_and(|details| details.accepts_zec);
        let problem = accepts.then(|| self.own_zec_ready().err()).flatten().map(NotReady::vendor);
        Some(json!({ "seller": store.name, "own": true, "acceptsZec": accepts, "problem": problem }))
    }
}
