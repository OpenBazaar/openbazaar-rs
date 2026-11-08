use bitcoin::secp256k1::{Keypair, Secp256k1, SecretKey};

use super::support::*;
use crate::{read_published_ruling, ModeratorRuling, Party};

#[test]
fn the_old_moderator_ruling_v2_still_verifies() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"never shipped").unwrap();
    let ruling = rule(&letter, DEADLINE).unwrap();
    let again = ModeratorRuling::from_bytes(&ruling.to_bytes()).unwrap();
    again.verify(&moderator_keys().signing_public()).unwrap();
    assert_eq!(again.favored(), Party::Buyer);
    assert_eq!(again.to_bytes()[..4], *b"OBRU");
    assert_eq!(again.to_bytes()[4], 2);
}

#[test]
fn a_buyer_with_the_published_record_can_tell_who_was_paid_for_that_store_and_listing() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"never shipped").unwrap();
    let ruling = rule(&letter, DEADLINE).unwrap();
    let ruling_bytes = ruling.to_bytes();
    let store = SecretKey::from_slice(&[0x91; 32]).unwrap();
    let store_key = Keypair::from_secret_key(&Secp256k1::new(), &store).x_only_public_key().0.serialize();
    let listing_id = [0x9a; 16];
    let record = moderator_keys().sign_public_ruling(&ruling_bytes, store_key, listing_id).unwrap();
    let published = read_published_ruling(&ruling_bytes, &record, &moderator_keys().signing_public()).unwrap();
    assert_eq!(published.favored, Party::Buyer, "the ruling says the buyer was paid");
    assert_eq!(published.store_key, store_key);
    assert_eq!(published.listing_id, listing_id);
    let per_order = buyer.terms().vendor;
    assert_ne!(published.store_key.as_slice(), &per_order[1..], "the store key is not the per-order vendor key");
}
