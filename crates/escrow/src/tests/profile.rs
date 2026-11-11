use openbazaar_protocol::{ModeratorProfile, SignedModeratorProfile};

use super::support::*;
use crate::{read_profile, review_letter, Error, LetterCache, ModeratorKeys, ModeratorRecord};

fn profile(record: &ModeratorRecord) -> ModeratorProfile {
    ModeratorProfile {
        moderator_key: record.id(),
        updated: 1_794_000_000,
        name: "Harbor Arbitration".into(),
        description: "Disputes about physical goods.".into(),
        terms: "Both sides send tracking and photos.".into(),
        languages: vec!["en".into()],
        address: format!("http://{}.onion", "a".repeat(56)),
        mailbox_key: [7; 32],
        record: record.to_bytes(),
    }
}

#[test]
fn a_record_round_trips_and_an_invalid_one_is_refused() {
    let record = record();
    assert_eq!(ModeratorRecord::from_bytes(&record.to_bytes()).unwrap(), record);
    assert_eq!(record.id().as_slice(), &record.signing_key[1..]);
    let mut same_keys = record.clone();
    same_keys.encryption_key = same_keys.signing_key;
    assert!(ModeratorRecord::from_bytes(&same_keys.to_bytes()).is_err());
    let mut trailing = record.to_bytes();
    trailing.push(0);
    assert_eq!(ModeratorRecord::from_bytes(&trailing), Err(Error::Malformed));
}

#[test]
fn a_profile_signed_by_the_ruling_key_reads_back_with_its_record() {
    let signed = moderator_keys().sign_profile(profile(&record())).unwrap();
    let read = read_profile(&signed.bytes).unwrap();
    assert_eq!(read, signed);
    assert_eq!(read.record, record());
    assert_eq!(read.id(), record().id());
}

#[test]
fn a_tampered_profile_is_refused() {
    let signed = moderator_keys().sign_profile(profile(&record())).unwrap();
    let decoded = SignedModeratorProfile::decode(&signed.bytes).unwrap();
    let mut cheaper = record();
    cheaper.fee = bitcoin::Amount::from_sat(MODERATOR_FEE.to_sat() + 1);
    for edit in [
        (|p: &mut ModeratorProfile, _: &ModeratorRecord| p.name = "Harbour Arbitration".into())
            as fn(&mut ModeratorProfile, &ModeratorRecord),
        |p, _| p.address = format!("http://{}.onion", "b".repeat(56)),
        |p, _| p.mailbox_key = [8; 32],
        |p, other| p.record = other.to_bytes(),
    ] {
        let mut changed = decoded.clone();
        edit(&mut changed.terms, &cheaper);
        assert_eq!(read_profile(&changed.encode().unwrap()), Err(Error::BadSignature));
    }
}

#[test]
fn a_profile_cannot_carry_another_moderators_record() {
    // Signing refuses a record for other keys.
    assert!(moderator_keys().sign_profile(profile(&other_record())).is_err());
    let mut named_other = profile(&record());
    named_other.moderator_key = other_record().id();
    assert!(moderator_keys().sign_profile(named_other).is_err());
    // A profile the other moderator signs itself, around this moderator's
    // record, verifies as a signature and is still refused.
    let other = ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED).unwrap();
    let mut borrowed = profile(&record());
    borrowed.moderator_key = other_record().id();
    let secret = bitcoin::secp256k1::SecretKey::from_slice(&other.signing().serialize()).unwrap();
    let forged = openbazaar_protocol::Signed::sign(borrowed, &secret).unwrap();
    forged.verify().unwrap();
    assert_eq!(read_profile(&forged.encode().unwrap()), Err(Error::ModeratorMismatch));
}

#[test]
fn a_letter_is_reviewed_without_the_moderators_secrets() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"never shipped").unwrap();
    let cache = LetterCache::default();
    let moderator = moderator();
    let chain = moderator.chain();
    let case = review_letter(&record(), chain, &cache, &letter, DEADLINE, &mut |_, _| Ok(())).unwrap();
    assert_eq!(case.memo(), b"never shipped");
    assert_eq!(
        review_letter(&other_record(), chain, &cache, &letter, DEADLINE, &mut |_, _| Ok(())).unwrap_err(),
        Error::ModeratorMismatch
    );
}
