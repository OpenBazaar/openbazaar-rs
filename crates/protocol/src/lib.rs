//! Signed store and listing records, plus the private order and dispute types.
//!
//! A listing's address is the hash of its canonical body. Orders are not
//! catalog records. Payment and escrow values in this crate are types only.

#![forbid(unsafe_code)]

mod content;
mod dispute;
mod encode;
mod error;
mod escrow;
mod fixture_labels;
mod fixtures;
mod keys;
mod labeler;
mod listing;
pub mod public_text;
mod order;
mod payment;
mod sign;
mod store;

pub use content::{ContentLabel, ContentRating, ContentTags};
pub use dispute::{DisputeEnvelope, DisputePlaintext};
pub use error::Error;
pub use escrow::{EscrowBranch, EscrowTerms};
pub use fixture_labels::{fixture_label_sets, fixture_labeler_key};
pub use fixtures::{
    fixture_catalog, fixture_dispute, fixture_keys, fixture_media, fixture_order, fixture_store_keys,
    package_rate, Catalog, FixtureKeys, FixtureStoreKeys, Moderator,
};
pub use keys::KeyRole;
pub use labeler::{resolve_labels, LabelEntry, LabelSet, LabelTarget, ResolvedLabel};
pub use listing::{Listing, ListingHash, ListingKind};
pub use order::{OrderEnvelope, OrderPlaintext};
pub use payment::DirectPay;
pub use store::{CountryCode, Store, StoreDetails, StoreId, StoreProfile};
