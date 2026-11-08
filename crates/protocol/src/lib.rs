//! Signed store and listing records, plus the private order and dispute types.
//!
//! A listing's address is the hash of its canonical body. Orders are not
//! catalog records. Payment and escrow values in this crate are types only.

#![forbid(unsafe_code)]

mod commerce;
#[cfg(test)]
mod commerce_tests;
mod content;
mod coupon;
mod dispute;
mod encode;
mod error;
mod escrow;
mod fixture_labels;
mod fixtures;
mod keys;
mod labeler;
mod listing;
mod listing_details;
#[cfg(test)]
mod listing_v6_tests;
pub mod public_text;
mod offer;
mod order;
mod payment;
pub mod pricing;
#[cfg(test)]
mod pricing_tests;
mod reputation;
mod review;
mod shipping;
mod sign;
mod signed;
mod store;
mod taxonomy;
mod variants;

pub use commerce::{Availability, Commerce, Condition, Pricing, Purchase, Returns, Sale, Tier, MAX_SPECIFICS, MAX_TIERS};
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
pub use listing_details::{FiatPrice, ListingDetails, ListingId, MAX_IMAGES};
pub use coupon::{Coupon, Discount, SignedCoupon, MAX_COUPON_LISTINGS};
pub use offer::{Bid, Offer, SignedOffer};
pub use order::{OrderEnvelope, OrderPlaintext};
pub use reputation::{
    accept_bond, bond_script, ruling_digest, Bond, BondLock, PublicRuling, SignedBond, SignedPublicRuling,
    MAINNET_BOND_REFUSAL, MIN_BOND_LOCK, MIN_BOND_SATS, NETWORK_MAINNET, NETWORK_TESTNET4,
};
pub use review::{Receipt, Reply, Review, SignedReceipt, SignedReply, SignedReview, MAX_REVIEW_TEXT};
pub use shipping::{Service, Shipping, MAX_SERVICES, MAX_SERVICE_REGIONS};
pub use signed::{Signed, Terms};
pub use taxonomy::{taxonomy_name, TAXONOMY};
pub use variants::{Axis, Variant, VariantId, Variants, MAX_AXES, MAX_AXIS_VALUES, MAX_VARIANTS};
pub use payment::DirectPay;
pub use store::{CountryCode, Store, StoreDetails, StoreId, StoreProfile, MAX_NAME};
