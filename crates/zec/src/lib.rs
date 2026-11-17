//! Direct Zcash payments for OpenBazaar.
//!
//! Built on upstream librustzcash: `zcash_keys` for ZIP 32 keys and Unified
//! Addresses, `zcash_client_backend` for scanning compact blocks, `orchard`
//! for the note format, `zip321` for payment URIs. This crate adds what those
//! libraries leave to a wallet: state that can be sealed in the node's vault
//! (scanned height, block hashes, received notes, the next address), matching
//! payments to orders, reorg handling, and a mock chain for tests.
//!
//! Ironwood is the pool that spends: every per-order address carries one
//! Orchard-protocol receiver (ZIP 316's only receiver type for that
//! protocol), a payment to it lands in Ironwood since NU6.3, and only
//! Ironwood notes pay an order. Orchard is outbound only; the wallet still
//! reads it, so notes from before NU6.3 restored with a phrase, and their
//! spends, show. Moving those notes into Ironwood is a pool migration, not
//! a payment.
//!
//! What never happens here:
//! - No server learns which outputs are ours. The chain source hands over
//!   compact blocks, and trial decryption runs here. The wallet never asks
//!   for a transaction by id.
//! - No SQLite and no second Tor client (see `docs/zcash.md`).
//! - No escrow. Zcash is direct pay only; the node refuses it for escrow.

#![forbid(unsafe_code)]

pub mod amount;
mod backend;
mod chain;
mod indexer;
pub mod keys;
#[cfg(feature = "mock")]
pub mod mock;
mod network;
mod send;
mod trees;
mod tx;
mod uri;
mod wallet;

pub use backend::{BackendStatus, IndexerConfig, Server, ServerKind};
pub use chain::{CommitmentTrees, SharedTip, ZecChain};
pub use send::{quote_payment, sign_payment, PaymentQuote, SendError, SignedPayment, SpendKey};
pub use indexer::IndexerChain;
pub use keys::{is_order_address, ViewKey};
pub use network::ZecNetwork;
pub use uri::zcash_uri;
pub use wallet::{Balance, Pool, Received, SyncStep, Unspent, ZecWallet, CONFIRMATIONS};

/// The compact block format of the lightwalletd protocol, which Zaino serves too.
pub mod proto {
    pub use zcash_client_backend::proto::compact_formats::{
        ChainMetadata, CompactBlock, CompactOrchardAction, CompactTx,
    };
}
