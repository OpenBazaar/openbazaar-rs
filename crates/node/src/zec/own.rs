//! The node's own store on Zcash.
//!
//! Its wallet is the node wallet's phrase (`wallets/node`, ZIP 32 account 0),
//! kept like a demo store's: the viewing key in `zec-watch.obv` in the receive
//! tier, so a locked node scans and can hand out addresses. The choice is
//! bit 2 of the record other nodes follow, so changing it needs the store key
//! (an unlocked wallet), and `store-record.obv` keeps it for the next boot.
//! Nothing can pay the store in Zcash yet: a sale from another node carries
//! bitcoin terms only.

use openbazaar_protocol::{ContentRating, Store};
use serde_json::{json, Value};

use super::AcceptError;
use crate::network::NetworkMode;
use crate::own_store::Rails;
use crate::Node;

pub(crate) const TESTNET4_ONLY: &str = "Your own store takes Zcash on testnet4 only for now. Mainnet follows once the node checks its Zcash indexer.";
pub(crate) const NEEDS_INDEXER: &str = "Zcash is off on this node. Start it with --zec-indexer.";

/// Whether the own store may sign the Zcash flag on `network`. Testnet4 only
/// until the indexer checks are in; return `true` to open mainnet.
pub(crate) fn own_store_allowed(network: NetworkMode) -> bool {
    network == NetworkMode::Testnet4
}

impl Node {
    /// Opt the own store in or out by signing its record again. Turning it off is never refused for the network or the indexer.
    pub(super) fn set_own_accepts_zec(&self, store: &Store, accept: bool) -> Result<(), AcceptError> {
        if accept && !own_store_allowed(self.network) {
            return Err(AcceptError::Mainnet);
        }
        if accept && self.zec.chain().is_none() {
            return Err(AcceptError::Off);
        }
        let secret = self.store_secret(&store.id).ok_or(AcceptError::Locked)?;
        let rails = Rails { zec: accept, ..Rails::of(Some(store)) };
        self.publish_own(store.rating.unwrap_or(ContentRating::General), rails, &secret).map_err(|err| AcceptError::Failed(err.to_string()))
    }

    /// The own store's row in `GET /zec/stores`, once it has a record.
    pub(super) fn own_zec_value(&self) -> Option<Value> {
        let store = self.own_record()?;
        Some(json!({ "seller": store.name, "own": true, "acceptsZec": store.details.is_some_and(|details| details.accepts_zec) }))
    }
}
