use bitcoin::Amount;

use super::support::*;
use crate::crypto::random_scalar;
use crate::{
    is_equivocation, Error, ModeratorRuling, Party, Phase, ReleaseSignature, SignedClaim,
    SignedRefund, VerifiableShare, ANCHOR_VALUE,
};

#[test]
fn stalling_buyer_loses_at_the_vendor_claim_height() {
    let (mut buyer, mut vendor) = funded();
    let mut chain = Chain::at(CLAIM - 1);
    assert!(matches!(
        vendor.broadcast_claim(chain.next_height() - 1),
        Err(Error::LockTimeNotSatisfied { .. })
    ));
    assert_eq!(vendor.phase(), Phase::Funded);
    assert!(chain.mine(vendor.claim().transaction()).is_err());

    // Height CLAIM + 1: the claim is final, the refund is not.
    chain.advance_to(CLAIM);
    assert!(buyer.broadcast_refund(chain.next_height()).is_err());
    let claim = vendor.broadcast_claim(chain.next_height()).unwrap();
    chain.mine(&claim).unwrap();
    assert_eq!(vendor.phase(), Phase::Claimed);

    // Later, the refund conflicts with the confirmed claim.
    chain.advance_to(REFUND);
    let refund = buyer.broadcast_refund(chain.next_height()).unwrap();
    assert_eq!(chain.mine(&refund), Err("double spend"));
}

#[test]
fn buyer_who_disputes_in_time_wins() {
    let (mut buyer, vendor) = funded();
    let mut chain = Chain::at(DEADLINE);
    let letter = buyer.dispute_letter(b"never shipped").unwrap();
    let ruling = rule(&letter, chain.tip).unwrap();
    let ruling = ModeratorRuling::from_bytes(&ruling.to_bytes()).unwrap();

    let won = buyer.settle_dispute(&ruling).unwrap();
    assert_eq!(buyer.phase(), Phase::Disputed);
    buyer.verify_key_spend(&won).unwrap();
    assert_eq!(won.output[0].script_pubkey, buyer.terms().buyer_payout);
    assert_eq!(
        won.output[0].value,
        FUNDING - NETWORK_FEE - MODERATOR_FEE - ANCHOR_VALUE
    );
    chain.mine(&won).unwrap();

    chain.advance_to(CLAIM);
    let claim = vendor
        .claim()
        .broadcastable_at(chain.next_height())
        .unwrap();
    assert_eq!(chain.mine(&claim), Err("double spend"));
}

#[test]
fn letters_after_the_deadline_are_refused() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"late").unwrap();
    assert_eq!(
        rule(&letter, DEADLINE + 1).unwrap_err(),
        Error::DisputeWindowClosed
    );
}

#[test]
fn vendor_can_win_a_dispute_and_buyer_cannot_use_its_ruling() {
    let (mut buyer, mut vendor) = funded();
    let letter = vendor.dispute_letter(b"buyer received it").unwrap();
    let ruling = rule(&letter, TIP).unwrap();
    assert_eq!(ruling.favored(), Party::Vendor);

    assert_eq!(
        buyer.settle_dispute(&ruling).unwrap_err(),
        Error::BadPayload
    );
    assert_eq!(buyer.phase(), Phase::Funded);

    let won = vendor.settle_dispute(&ruling).unwrap();
    vendor.verify_key_spend(&won).unwrap();
    assert_eq!(won.output[0].script_pubkey, vendor.terms().vendor_payout);
    assert_eq!(won.input[0].witness.nth(0).unwrap().len(), 64);
}

#[test]
fn moderator_share_alone_does_not_finish_a_branch() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"memo").unwrap();
    let ruling = rule(&letter, TIP).unwrap();
    let branch = buyer.branch();
    let adapted = branch
        .adaptor_sig
        .adapt::<musig2::LiftedSignature>(ruling.share());
    let sighash = buyer.dispute_outcome(Party::Buyer).clone();
    let sighash = crate::tx::key_spend_sighash(&sighash, buyer.funding_output()).unwrap();
    let ok = adapted.is_some_and(|sig| {
        crate::crypto::bip340_verify(&buyer.output_key(), &sig.serialize(), &sighash).is_ok()
    });
    assert!(
        !ok,
        "the moderator half alone must not produce a valid signature"
    );
}

#[test]
fn rulings_are_signed_and_equivocation_is_provable() {
    let (mut buyer, vendor) = funded();
    // A moderator who bypasses its ruling store (two fresh stores) can sign both.
    let for_buyer = rule(&buyer.dispute_letter(b"a").unwrap(), TIP).unwrap();
    let for_vendor = rule(&vendor.dispute_letter(b"b").unwrap(), TIP).unwrap();
    let moderator = record().signing_key;
    for_buyer.verify(&moderator).unwrap();
    assert!(for_buyer.verify(&record().encryption_key).is_err());
    assert!(for_buyer.verify(&other_record().signing_key).is_err());
    assert!(is_equivocation(&for_buyer, &for_vendor, &moderator));
    assert!(!is_equivocation(&for_buyer, &for_buyer, &moderator));

    let mut forged = for_buyer.to_bytes();
    let last = forged.len() - 1;
    forged[last] ^= 1;
    let forged = ModeratorRuling::from_bytes(&forged).unwrap();
    assert_eq!(
        buyer.settle_dispute(&forged).unwrap_err(),
        Error::BadSignature
    );
    buyer.settle_dispute(&for_buyer).unwrap();
}

#[test]
fn moderator_fee_appears_only_in_dispute_outcomes_and_is_signed() {
    let (mut buyer, vendor) = funded();
    let moderator_script = buyer.terms().moderator.payout.clone();
    let pays_moderator = |tx: &bitcoin::Transaction| {
        tx.output
            .iter()
            .any(|out| out.script_pubkey == moderator_script)
    };

    assert!(!pays_moderator(buyer.refund().transaction()));
    assert!(!pays_moderator(vendor.claim().transaction()));
    let release = release(&mut buyer.duplicate(), &mut vendor.duplicate());
    assert!(!pays_moderator(&release));
    assert_eq!(release.output.len(), 1);
    assert_eq!(release.output[0].value, FUNDING - NETWORK_FEE);

    for favored in [Party::Buyer, Party::Vendor] {
        let outcome = buyer.dispute_outcome(favored);
        assert_eq!(outcome.output.len(), 3);
        assert_eq!(outcome.output[1].script_pubkey, moderator_script);
        assert_eq!(outcome.output[1].value, MODERATOR_FEE);
    }

    let letter = buyer.dispute_letter(b"x").unwrap();
    let ruling = rule(&letter, TIP).unwrap();
    let won = buyer.settle_dispute(&ruling).unwrap();
    let mut skimmed = won.clone();
    skimmed.output[1].value = Amount::from_sat(1);
    skimmed.output[0].value += MODERATOR_FEE - Amount::from_sat(1);
    assert_eq!(buyer.verify_key_spend(&skimmed), Err(Error::BadSignature));
    let mut dropped = won;
    dropped.output.truncate(1);
    assert_eq!(buyer.verify_key_spend(&dropped), Err(Error::BadSignature));
}

#[test]
fn letters_are_self_verifying() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"cover arrived torn").unwrap();
    let case = moderator().review(&letter, TIP).unwrap();
    assert_eq!(case.favored(), Party::Buyer);
    assert_eq!(case.memo(), b"cover arrived torn");
    assert_eq!(case.outcome(), buyer.dispute_outcome(Party::Buyer));
    assert_eq!(case.terms().moderator.fee, MODERATOR_FEE);

    let mut stranger = crate::Moderator::new(
        crate::ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED).unwrap(),
        other_record(),
        std::collections::HashMap::from([(outpoint(), buyer.funding_output().clone())]),
        crate::InMemoryRulingStore::default(),
    )
    .unwrap();
    assert_eq!(
        stranger.open(&letter, TIP).unwrap_err(),
        Error::ModeratorMismatch
    );

    // Flip one byte at a time across the letter; nothing may still open.
    let step = letter.len() / 97;
    for at in (0..letter.len()).step_by(step.max(1)) {
        let mut tampered = letter.clone();
        tampered[at] ^= 0x01;
        assert!(
            rule(&tampered, TIP).is_err(),
            "byte {at} of {} was not covered",
            letter.len()
        );
    }
}

#[test]
fn a_claimant_signed_letter_with_a_bad_branch_is_refused() {
    let (buyer, _) = funded();
    let open = |letter: &[u8]| rule(letter, TIP).map(|_| ());

    let garbage = buyer.forged_letter(|branch| branch.share.corrupt_bit(3));
    assert_eq!(open(&garbage), Err(Error::BadProof));

    // The adaptor pre-signature must verify against T_fav + T_mod for this sighash.
    let wrong_half = buyer.forged_letter(|branch| branch.party_point = branch.share.point());
    assert_eq!(open(&wrong_half), Err(Error::BadSignature));

    // A well-formed share the vendor never signed over changes the adaptor point.
    let moderator = musig2::secp::Point::from_slice(&record().encryption_key).unwrap();
    let fresh_share = buyer.forged_letter(|branch| {
        branch.share = VerifiableShare::encrypt(random_scalar(), &moderator, &[0; 32]).unwrap();
    });
    assert_eq!(open(&fresh_share), Err(Error::BadSignature));
}

#[test]
fn happy_path_release_needs_both_parties() {
    let (mut buyer, mut vendor) = funded();
    let tx = release(&mut buyer, &mut vendor);
    vendor.verify_key_spend(&tx).unwrap();
    assert_eq!(tx.output[0].script_pubkey, vendor.terms().vendor_payout);
    assert_eq!(tx.lock_time, bitcoin::absolute::LockTime::ZERO);
    assert_eq!(buyer.phase(), Phase::ReleaseSigned);
    assert_eq!(vendor.phase(), Phase::Completed);
    assert!(matches!(
        buyer.broadcast_refund(REFUND + 1),
        Err(Error::InvalidTransition { .. })
    ));
    assert!(matches!(
        vendor.broadcast_claim(CLAIM + 1),
        Err(Error::InvalidTransition { .. })
    ));

    // A forged buyer partial is rejected and the vendor stays funded.
    let (mut buyer, mut vendor) = funded();
    let nonce = vendor.release_nonce().unwrap();
    let mut forged: ReleaseSignature = buyer.sign_release(nonce.public()).unwrap();
    forged.partial = musig2::PartialSignature::from_slice(&[1; 32]).unwrap();
    assert_eq!(
        vendor.complete_release(nonce, &forged).unwrap_err(),
        Error::BadPartial
    );
    assert_eq!(vendor.phase(), Phase::Funded);
}

fn release(
    buyer: &mut crate::BuyerEscrow,
    vendor: &mut crate::VendorEscrow,
) -> bitcoin::Transaction {
    let nonce = vendor.release_nonce().unwrap();
    let signature = buyer.sign_release(nonce.public()).unwrap();
    let signature = ReleaseSignature::from_bytes(&signature.to_bytes()).unwrap();
    vendor.complete_release(nonce, &signature).unwrap()
}

#[test]
fn presigned_spends_persist_and_reverify() {
    let (buyer, vendor) = funded();
    let bytes = buyer.refund().to_bytes();
    let loaded = SignedRefund::from_bytes(&bytes).unwrap();
    assert_eq!(&loaded, buyer.refund());
    assert!(loaded.broadcastable_at(REFUND).is_err());
    assert_eq!(
        loaded.broadcastable_at(REFUND + 1).unwrap(),
        *buyer.refund().transaction()
    );

    let mut corrupt = bytes.clone();
    let at = corrupt.len() - 70;
    corrupt[at] ^= 1;
    assert!(SignedRefund::from_bytes(&corrupt).is_err());
    assert!(SignedClaim::from_bytes(&bytes).is_err());
    let claim = SignedClaim::from_bytes(&vendor.claim().to_bytes()).unwrap();
    assert_eq!(&claim, vendor.claim());
}

#[test]
fn protocol_dispute_envelope_carries_both_branches() {
    use openbazaar_protocol::{
        fixture_catalog, fixture_order, DisputeEnvelope, DisputePlaintext, OrderEnvelope,
    };

    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"torn").unwrap();
    let terms = buyer.envelope_terms(letter.clone()).unwrap();
    assert_ne!(
        terms.buyer_wins.adaptor_point,
        terms.vendor_wins.adaptor_point
    );
    assert!(terms.vendor_wins.letter.is_empty());

    let identity = bitcoin::secp256k1::SecretKey::from_slice(&[0x66; 32]).unwrap();
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let opener = identity.x_only_public_key(&secp).0.serialize();
    let OrderEnvelope::Plaintext { order, .. } = fixture_order(&fixture_catalog()) else {
        panic!("fixture order is plaintext");
    };
    let envelope = DisputeEnvelope::sign(
        DisputePlaintext {
            opener,
            order,
            claim: "torn".into(),
            escrow: terms,
        },
        &identity,
    )
    .unwrap();
    let decoded = DisputeEnvelope::decode(&envelope.encode().unwrap()).unwrap();
    decoded.verify().unwrap();
    let DisputeEnvelope::Plaintext { dispute, .. } = decoded else {
        panic!("plaintext envelope");
    };

    let case = moderator()
        .review(&dispute.escrow.buyer_wins.letter, TIP)
        .unwrap();
    case.check_envelope(&dispute.escrow).unwrap();

    let mut swapped = dispute.escrow.clone();
    std::mem::swap(
        &mut swapped.buyer_wins.adaptor_point,
        &mut swapped.vendor_wins.adaptor_point,
    );
    assert_eq!(case.check_envelope(&swapped), Err(Error::BadPayload));
    let mut cheap = dispute.escrow;
    cheap.moderator_fee = 1;
    assert_eq!(case.check_envelope(&cheap), Err(Error::BadPayload));
}
