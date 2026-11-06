//! Direct Bitcoin payment rails for OpenBazaar.
//!
//! A listing is paid in satoshis on one of two rails, side by side:
//!
//! * Lightning, as a BOLT 12 offer.
//! * On-chain, as a BIP 352 silent-payment code, for a buyer with no Lightning.
//!
//! When both wallets support it, the on-chain payment is Payjoin v2 (BIP 77):
//! the vendor adds an input. There is no swap service, coinjoin, ecash, Ark, or
//! exchange-rate API in this crate.
//!
//! The home node still has to scan for silent-payment outputs and run LDK to
//! answer the offer. The phone wallet still has to build and sign the on-chain
//! transaction while the app is open. This library is what those apps call.

#![forbid(unsafe_code)]

mod direct;
mod error;
mod offer;
mod payjoin;
mod silent;

pub use ::payjoin::receive::InputPair;
pub use direct::DirectPay;
pub use error::PayError;
pub use offer::Bolt12Offer;
pub use payjoin::{
    contribute_vendor_input, ContributedProposal, EncapsulatedRequest, ObliviousDirectory,
    PayjoinReceiver, PayjoinSender,
};
pub use silent::SilentPaymentCode;
