//! Signed store and listing records, plus the private order and dispute types.
//!
//! A listing's address is the hash of its canonical body. Orders are not
//! catalog records. Payment and escrow values in this crate are types only.

#![forbid(unsafe_code)]

mod dispute;
mod encode;
mod error;
mod escrow;
mod fixtures;
mod keys;
mod listing;
mod order;
mod payment;
mod sign;
mod store;

pub use dispute::{DisputeEnvelope, DisputePlaintext};
pub use error::Error;
pub use escrow::EscrowTerms;
pub use fixtures::{
    fixture_catalog, fixture_dispute, fixture_keys, fixture_order, package_rate, vendor_mail_key,
    Catalog, FixtureKeys, Moderator,
};
pub use keys::KeyRole;
pub use listing::{Listing, ListingHash, ListingKind};
pub use order::{OrderEnvelope, OrderPlaintext};
pub use payment::DirectPay;
pub use store::{Store, StoreId};
