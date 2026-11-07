//! Direct Lightning payments for OpenBazaar.
//!
//! One embedded LDK node ([`ldk_node`]) per OpenBazaar node, behind the small
//! [`LightningRail`] trait so the node crate never touches LDK types. The
//! seller's node issues a BOLT 11 invoice per order; when it has no channel
//! with room for the payment, the invoice routes through a Lightning Service
//! Provider (LSP) that opens a just-in-time channel (LSPS2) as the payment
//! arrives. Escrow never runs over Lightning.
//!
//! - Keys: one 32-byte secret from the wallet seed (the node crate derives
//!   it on its own path); [`keys`] turns it into LDK's 64-byte entropy and
//!   the key that seals Lightning state.
//! - State: LDK's key-value store, sealed value by value with
//!   ChaCha20-Poly1305 ([`sealed_store`]).
//! - Network: whichever [`LnConfig`] names; the node crate picks it.
//!
//! The LSP sees every payment to the seller (amount, time, and the payment
//! hash) and the channel it opens; it must be reachable for a new channel.
//! See `docs/lightning.md`.

pub mod config;
pub mod keys;
mod ldk;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod rail;
#[cfg(feature = "regtest")]
pub mod regtest;
pub mod sealed_store;

pub use config::{ChainSource, GossipSource, LnConfig, LspConfig, PeerAddress};
pub use ldk::LdkRail;
pub use ldk_node;
pub use rail::{
    decode_invoice, Balances, Channel, Incoming, InvoiceInfo, IssuedInvoice, LightningRail, LnError, NodeInfo, Outgoing,
};
