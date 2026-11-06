//! Direct Monero payments for OpenBazaar.
//!
//! Built on monero-oxide's `monero-wallet` (Serai's Monero library, audited
//! by Cypher Stack in 2025): view-tag scanning, subaddresses, CLSAG and
//! Bulletproofs+ sends, decoy selection. This crate adds what that library
//! leaves to its caller: wallet state (scanned height, outputs, burning-bug
//! and reorg checks), coin selection for one recipient, and a daemon client
//! that only ever talks to a monerod the user chose.
//!
//! What never happens here:
//! - No light-wallet server. Those need the private view key, so the node
//!   would tell a third party which payments are its own. The daemon client
//!   only sends the routes in [`transport::ROUTES`], and no request carries a
//!   key.
//! - No payment with more than one recipient. Change goes back to the wallet's
//!   own primary address.
//! - No escrow. Monero is direct pay only; the node refuses it for escrow.

#![forbid(unsafe_code)]

pub mod amount;
mod backend;
mod chain;
mod daemon;
mod decoys;
pub mod keys;
#[cfg(feature = "mock")]
pub mod mock;
mod network;
mod send;
pub mod transport;
mod uri;
mod wallet;

pub use backend::{BackendStatus, DaemonConfig, NodeKind};
pub use chain::XmrChain;
pub use daemon::DaemonChain;
pub use keys::{SpendKey, ViewKeys};
pub use network::XmrNetwork;
pub use send::{quote_payment, sign_payment, PaymentQuote, SendError, SignedPayment, RING_LEN};
pub use uri::monero_uri;
pub use wallet::{Balance, Received, SyncStep, XmrWallet, CONFIRMATIONS};

pub use monero_wallet::address::MoneroAddress;
pub use monero_wallet::interface::FeeRate;

/// Parse an address and require that it is on `network`.
pub fn address_for(text: &str, network: XmrNetwork) -> Result<MoneroAddress, String> {
    MoneroAddress::from_str(network.address_network(), text.trim())
        .map_err(|_| format!("That is not a Monero {network} address."))
}
