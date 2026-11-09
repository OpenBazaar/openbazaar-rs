use bitcoin::Amount;

use super::support::*;
use crate::crypto::random_scalar;
use crate::{
    is_equivocation, Error, EscrowTerms, ModeratorRuling, Party, Phase, ReleaseSignature,
    SignedClaim, SignedRefund, SignedTerms, VerifiableShare, ANCHOR_VALUE,
};

/// A missing moderator must not pay the vendor. The presigned claim is the
/// buyer refund: it is not final in the old claim window, it pays the buyer,
/// and it does not pay the moderator fee.
#[test]
fn a_missing_moderator_does_not_pay_the_vendor() {
    let (mut buyer, mut vendor) = funded();
    let claim_tx = vendor.claim().transaction().clone();
    assert_eq!(claim_tx.output, buyer.refund().transaction().output);
    assert_eq!(claim_tx.lock_time, buyer.refund().transaction().lock_time);
    assert_eq!(claim_tx.output[0].script_pubkey, buyer.terms().buyer_payout);
    assert_ne!(
        claim_tx.output[0].script_pubkey,
        vendor.terms().vendor_payout
    );
    assert!(claim_tx
        .output
        .iter()
        .all(|out| out.script_pubkey != buyer.terms().moderator.payout));
    assert_eq!(
        claim_tx.output[0].value,
        FUNDING - NETWORK_FEE - ANCHOR_VALUE
    );

    let mut chain = Chain::at(CLAIM);
    assert!(matches!(
        vendor.broadcast_claim(chain.next_height()),
        Err(Error::LockTimeNotSatisfied { .. })
    ));
    assert_eq!(vendor.phase(), Phase::Funded);
    assert_eq!(chain.mine(&claim_tx), Err("non-final"));

    chain.advance_to(REFUND);
    let refund = buyer.broadcast_refund(chain.next_height()).unwrap();
    assert_eq!(refund.output[0].script_pubkey, buyer.terms().buyer_payout);
    assert!(refund
        .output
        .iter()
        .all(|out| out.script_pubkey != vendor.terms().vendor_payout));
    chain.mine(&refund).unwrap();
    let claim = vendor.broadcast_claim(chain.next_height()).unwrap();
    assert_eq!(claim.output, refund.output);
    assert_eq!(chain.mine(&claim), Err("double spend"));
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

    // The claim is the buyer refund, so it is not final in the old claim window.
    chain.advance_to(CLAIM);
    assert!(matches!(
        vendor.claim().broadcastable_at(chain.next_height()),
        Err(Error::LockTimeNotSatisfied { .. })
    ));
    chain.advance_to(REFUND);
    let claim = vendor
        .claim()
        .broadcastable_at(chain.next_height())
        .unwrap();
    assert_eq!(claim.output[0].script_pubkey, buyer.terms().buyer_payout);
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
    assert_eq!(
        won.output[0].value,
        FUNDING - NETWORK_FEE - MODERATOR_FEE - ANCHOR_VALUE
    );
    assert_eq!(won.output[1].script_pubkey, vendor.terms().moderator.payout);
    assert_eq!(won.output[1].value, MODERATOR_FEE);
    assert_eq!(ruling.to_bytes()[4], 2, "ModeratorRuling v2 still verifies");
    ModeratorRuling::from_bytes(&ruling.to_bytes())
        .unwrap()
        .verify(&moderator_keys().signing_public())
        .unwrap();
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
fn a_vendor_ruling_pays_the_vendor_and_the_claim_does_not() {
    let (mut buyer, mut vendor) = funded();
    let moderator_script = buyer.terms().moderator.payout.clone();
    let pays_moderator = |tx: &bitcoin::Transaction| {
        tx.output
            .iter()
            .any(|out| out.script_pubkey == moderator_script)
    };

    assert!(!pays_moderator(buyer.refund().transaction()));
    let release = release(&mut buyer.duplicate(), &mut vendor.duplicate());
    assert!(!pays_moderator(&release));
    assert_eq!(release.output.len(), 1);
    assert_eq!(release.output[0].value, FUNDING - NETWORK_FEE);

    let claim = vendor.claim().transaction().clone();
    assert!(!pays_moderator(&claim), "a missing moderator is not paid");
    assert_ne!(claim.output[0].script_pubkey, buyer.terms().vendor_payout);
    assert_eq!(claim.output, buyer.refund().transaction().output);

    for (outcome, favored) in [
        (buyer.dispute_outcome(Party::Buyer), Party::Buyer),
        (buyer.dispute_outcome(Party::Vendor), Party::Vendor),
    ] {
        assert_eq!(outcome.output.len(), 3);
        assert_eq!(
            &outcome.output[0].script_pubkey,
            buyer.terms().payout(favored)
        );
        assert_eq!(
            outcome.output[0].value,
            FUNDING - NETWORK_FEE - MODERATOR_FEE - ANCHOR_VALUE
        );
        assert_eq!(outcome.output[1].script_pubkey, moderator_script);
        assert_eq!(outcome.output[1].value, MODERATOR_FEE);
    }
    let ruling = rule(&vendor.dispute_letter(b"shipped").unwrap(), TIP).unwrap();
    assert_eq!(ruling.favored(), Party::Vendor);
    let won = vendor.settle_dispute(&ruling).unwrap();
    assert_eq!(won.output[0].script_pubkey, vendor.terms().vendor_payout);
    assert_eq!(
        won.output[0].value,
        buyer.dispute_outcome(Party::Vendor).output[0].value
    );
    assert!(pays_moderator(&won));

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
fn terms_are_version_4_and_reject_a_version_3_encoding() {
    let order = order(ORDER_SECRET);
    let encoded = order.terms.encode();
    assert_eq!(&encoded[..4], b"OBET");
    assert_eq!(encoded[4], 4);
    assert_eq!(
        order.terms.digest(),
        crate::proofs::tagged("ob-escrow/terms/v4", &[&encoded])
    );
    assert_ne!(
        order.terms.digest(),
        crate::proofs::tagged("ob-escrow/terms/v3", &[&encoded])
    );
    let mut version_3 = encoded.clone();
    version_3[4] = 3;
    assert_eq!(EscrowTerms::decode(&version_3), Err(Error::Malformed));
    assert_eq!(EscrowTerms::decode(&encoded).unwrap(), order.terms);

    let signed = sign(&order).encode();
    assert_eq!(&signed[..4], b"OBTS");
    assert_eq!(signed[4], 4);
    let mut signed_v3 = signed.clone();
    signed_v3[4] = 3;
    assert_eq!(SignedTerms::decode(&signed_v3), Err(Error::Malformed));
    assert_eq!(SignedTerms::decode(&signed).unwrap().terms(), &order.terms);
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
        vendor.broadcast_claim(REFUND + 1),
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

/// The signed fee is paid to the moderator payout key already in the terms.
/// A larger fee after the buyer signed, or a name or street on that fee, is refused.
#[test]
fn the_moderator_key_is_paid_the_signed_fee_and_a_larger_fee_or_a_name_is_refused() {
    let order = order(ORDER_SECRET);
    let buyer_sig = order.terms.sign(&order.buyer_secret).unwrap();
    let vendor_sig = order.terms.sign(&order.vendor_secret).unwrap();
    let mut larger = order.terms.clone();
    larger.moderator.fee = MODERATOR_FEE + Amount::from_sat(1);
    assert_eq!(
        SignedTerms::new(larger, buyer_sig, vendor_sig).unwrap_err(),
        Error::BadTermsSignature,
        "a larger fee after the buyer signed is refused"
    );

    let (mut buyer, vendor) = funded();
    let key = buyer.terms().moderator.payout.clone();
    let signed_fee = buyer.terms().moderator.fee;
    assert_eq!(signed_fee, MODERATOR_FEE);
    assert!(
        key.is_p2tr(),
        "the fee is paid to the moderator key in the terms"
    );

    let ruling = rule(&buyer.dispute_letter(b"x").unwrap(), TIP).unwrap();
    let settled = buyer.settle_dispute(&ruling).unwrap();
    let paid: Vec<_> = settled
        .output
        .iter()
        .filter(|out| out.script_pubkey == key)
        .collect();
    assert_eq!(paid.len(), 1, "a ruling pays the moderator key once");
    assert_eq!(paid[0].value, signed_fee);
    let script = paid[0].script_pubkey.as_bytes();
    assert!(!script.windows(5).any(|window| window == b"Alice"));
    assert!(!script.windows(8).any(|window| window == b"12 Cedar"));
    assert!(
        vendor
            .claim()
            .transaction()
            .output
            .iter()
            .all(|out| out.script_pubkey != key),
        "a missing moderator is not paid from the claim"
    );

    let policy = crate::Policy::default();
    for label in [*b"Alice\0\0\0", *b"12 Cedar"] {
        let named = bitcoin::ScriptBuf::new_op_return(label);
        assert_eq!(
            moderator_keys()
                .record(named, MODERATOR_FEE, policy)
                .validate()
                .unwrap_err(),
            Error::NonStandardOutput,
            "a name or street cannot receive the fee"
        );
    }

    let clean = moderator_keys()
        .sign_public_ruling(&ruling.to_bytes(), [2; 32], [3; 16])
        .unwrap();
    assert!(!clean.windows(5).any(|window| window == b"Alice"));
    assert!(!clean.windows(8).any(|window| window == b"12 Cedar"));
    let buyer_key = buyer.terms().buyer;
    assert!(!clean
        .windows(buyer_key.len())
        .any(|window| window == buyer_key.as_slice()));

    let mut name = [0u8; 16];
    name[..5].copy_from_slice(b"Alice");
    assert!(
        moderator_keys()
            .sign_public_ruling(&ruling.to_bytes(), [2; 32], name)
            .is_err(),
        "a name on the public record of the fee is refused"
    );
    let mut street = [0u8; 16];
    street[..15].copy_from_slice(b"12 Cedar Street");
    assert!(
        moderator_keys()
            .sign_public_ruling(&ruling.to_bytes(), [4; 32], street)
            .is_err(),
        "a street on the public record of the fee is refused"
    );
}
