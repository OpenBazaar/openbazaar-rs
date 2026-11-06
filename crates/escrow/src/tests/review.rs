//! Regression tests for the adversarial review findings.

use std::collections::HashMap;

use bitcoin::hashes::Hash;
use bitcoin::{Amount, ScriptBuf, TxOut};

use super::support::*;
use crate::{
    anchor_script, is_equivocation, payout_dust_limit, BuyerSession, Context, Error, FundingPlan,
    ModeratorKeys, Party, Phase, Policy, VendorSession, ANCHOR_VALUE,
};

fn start(o: &Order, terms: crate::EscrowTerms, ctx: &Context) -> Result<(), Error> {
    BuyerSession::start(
        sign_terms(o, terms),
        &o.buyer_secret,
        ctx,
        FundingPlan::Outpoint(outpoint()),
    )
    .map(|_| ())
}

/// H1: the reviewer's repro used heights 1000/1144/1288 with the tip at
/// 900,000, so the vendor claim was broadcastable at 900,001.
#[test]
fn h1_past_or_too_close_heights_are_rejected_at_funding() {
    let o = order(ORDER_SECRET);
    let mut past = o.terms.clone();
    past.dispute_deadline = 1_000;
    past.vendor_claim_height = 1_144;
    past.refund_height = 1_288;
    past.validate(&Policy::default()).unwrap();
    assert_eq!(
        start(&o, past.clone(), &ctx()),
        Err(Error::HeightsOutOfRange)
    );

    let (_, open) = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &ctx(),
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();
    let err = VendorSession::accept(sign_terms(&o, past), &o.vendor_secret, &ctx(), &open)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err, Error::HeightsOutOfRange);

    let policy = Policy::default();
    let mut rushed = o.terms.clone();
    rushed.dispute_deadline = TIP + policy.min_delivery_window - 1;
    assert_eq!(
        start(&o, rushed.clone(), &ctx()),
        Err(Error::HeightsOutOfRange)
    );
    rushed.dispute_deadline += 1;
    start(&o, rushed, &ctx()).unwrap();

    let mut far = o.terms.clone();
    far.refund_height = TIP + policy.max_refund_horizon + 1;
    assert_eq!(
        start(&o, far.clone(), &ctx()),
        Err(Error::HeightsOutOfRange)
    );
    far.refund_height -= 1;
    start(&o, far, &ctx()).unwrap();
}

/// H2: every payout is a standard type above its own dust limit, and the
/// moderator payout and fee are exactly the published ones.
#[test]
fn h2_moderator_payout_must_be_standard_published_and_distinct() {
    let p2wpkh = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([3; 20]));
    assert_eq!(payout_dust_limit(&p2wpkh), Some(Amount::from_sat(294)));
    assert_eq!(payout_dust_limit(&p2tr(1)), Some(Amount::from_sat(330)));
    assert_eq!(payout_dust_limit(&ScriptBuf::new()), None);
    assert_eq!(payout_dust_limit(&ScriptBuf::new_op_return([1u8; 8])), None);
    assert_eq!(payout_dust_limit(&anchor_script()), None);

    let keys = moderator_keys();
    let policy = Policy::default();
    let bad_records = [
        keys.record(ScriptBuf::new(), MODERATOR_FEE, policy),
        keys.record(ScriptBuf::new_op_return([1u8; 8]), MODERATOR_FEE, policy),
        keys.record(ScriptBuf::from_bytes(vec![0x51]), MODERATOR_FEE, policy),
        keys.record(p2wpkh.clone(), Amount::from_sat(293), policy),
        keys.record(p2tr(0xb3), Amount::from_sat(329), policy),
    ];
    for bad in bad_records {
        assert_eq!(bad.validate(), Err(Error::NonStandardOutput), "{bad:?}");
    }
    keys.record(p2wpkh, Amount::from_sat(294), policy)
        .validate()
        .unwrap();

    // The terms must carry exactly the record the party trusts.
    let o = order(ORDER_SECRET);
    let mut skimmed = o.terms.clone();
    skimmed.moderator.fee = MODERATOR_FEE + Amount::from_sat(1);
    assert_eq!(start(&o, skimmed, &ctx()), Err(Error::ModeratorMismatch));
    let mut redirected = o.terms.clone();
    redirected.moderator.payout = o.terms.vendor_payout.clone();
    assert_eq!(
        start(&o, redirected.clone(), &ctx()),
        Err(Error::ModeratorMismatch)
    );

    // Even a record published that way cannot share a payout with a party.
    let colluding = Context {
        moderator: redirected.moderator.clone(),
        ..ctx()
    };
    assert!(matches!(
        start(&o, redirected, &colluding),
        Err(Error::InvalidParams(_))
    ));

    let mut odd_buyer = o.terms.clone();
    odd_buyer.buyer_payout = ScriptBuf::from_bytes(vec![0x51]);
    assert_eq!(start(&o, odd_buyer, &ctx()), Err(Error::NonStandardOutput));
}

/// H2 on the moderator side: a moderator only opens letters naming its own record.
#[test]
fn h2_moderator_refuses_letters_with_a_different_payout_or_fee() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"x").unwrap();
    let funding = HashMap::from([(outpoint(), buyer.funding_output().clone())]);
    for record in [
        moderator_keys().record(p2tr(0xb9), MODERATOR_FEE, Policy::default()),
        moderator_keys().record(p2tr(0xb3), Amount::from_sat(5_000), Policy::default()),
    ] {
        let mut m = crate::Moderator::new(
            moderator_keys(),
            record,
            funding.clone(),
            crate::InMemoryRulingStore::default(),
        )
        .unwrap();
        assert_eq!(m.open(&letter, TIP).unwrap_err(), Error::ModeratorMismatch);
    }
}

/// M1: rulings sign the terms digest and output key; the moderator checks the
/// funding output against its own chain view.
#[test]
fn m1_rulings_over_different_terms_are_not_equivocation() {
    let (buyer, _) = funded();
    let other = order([0x66; 32]);
    let (_, other_vendor) = run_funding(&other, FundingPlan::Outpoint(outpoint())).unwrap();

    let real = moderator()
        .open(&buyer.dispute_letter(b"a").unwrap(), TIP)
        .unwrap();
    // A second escrow reusing the outpoint in its own terms. On a real chain
    // only one exists; a moderator with a misconfigured view rules on both.
    let mut confused = moderator_with_chain(HashMap::from([(
        outpoint(),
        other_vendor.funding_output().clone(),
    )]));
    let framed = confused
        .open(&other_vendor.dispute_letter(b"b").unwrap(), TIP)
        .unwrap();
    assert_eq!(real.outpoint(), framed.outpoint());
    assert_ne!(real.favored(), framed.favored());
    assert_ne!(real.terms_digest(), framed.terms_digest());
    assert!(!is_equivocation(&real, &framed, &record().signing_key));
}

#[test]
fn m1_moderator_checks_the_funding_output_on_chain() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"a").unwrap();
    let mut unknown = moderator_with_chain(HashMap::new());
    assert_eq!(
        unknown.open(&letter, TIP).unwrap_err(),
        Error::FundingNotOnChain
    );

    let mut wrong_value = buyer.funding_output().clone();
    wrong_value.value -= Amount::from_sat(1);
    let mut lying = moderator_with_chain(HashMap::from([(outpoint(), wrong_value)]));
    assert_eq!(
        lying.open(&letter, TIP).unwrap_err(),
        Error::FundingNotOnChain
    );

    let wrong_script = TxOut {
        value: FUNDING,
        script_pubkey: p2tr(0xee),
    };
    let mut lying = moderator_with_chain(HashMap::from([(outpoint(), wrong_script)]));
    assert_eq!(
        lying.open(&letter, TIP).unwrap_err(),
        Error::FundingNotOnChain
    );

    moderator().open(&letter, TIP).unwrap();
}

/// M2: one moderator with one store hands out at most one branch per escrow.
#[test]
fn m2_moderator_refuses_a_second_branch() {
    let (buyer, vendor) = funded();
    let mut m = moderator();
    let first = m.open(&buyer.dispute_letter(b"a").unwrap(), TIP).unwrap();
    assert_eq!(first.favored(), Party::Buyer);
    assert_eq!(
        m.open(&vendor.dispute_letter(b"b").unwrap(), TIP)
            .unwrap_err(),
        Error::AlreadyRuled
    );
    // The same branch can be re-issued, e.g. after the claimant lost it.
    let again = m.open(&buyer.dispute_letter(b"c").unwrap(), TIP).unwrap();
    assert_eq!(again.favored(), Party::Buyer);
}

/// M5: the moderator's policy is published, signed into the terms, checked
/// by the parties at funding, and the only policy the moderator applies.
#[test]
fn m5_moderator_policy_is_published_and_checked_at_funding() {
    let o = order(ORDER_SECRET);
    let strict = Policy {
        min_fee_rate_sat_vb: 50,
        ..Policy::default()
    };
    let strict_record = moderator_keys().record(p2tr(0xb3), MODERATOR_FEE, strict);
    let mut terms = o.terms.clone();
    terms.moderator = strict_record.clone();
    let party_ctx = Context {
        moderator: strict_record.clone(),
        ..ctx()
    };
    // The party's own policy accepts the fee; the moderator's published one does not.
    assert_eq!(start(&o, terms.clone(), &party_ctx), Err(Error::FeeTooLow));

    // Terms that fund under the published policy are accepted by the moderator.
    let lenient = Policy {
        min_fee_rate_sat_vb: 1,
        ..Policy::default()
    };
    let lenient_record = moderator_keys().record(p2tr(0xb3), MODERATOR_FEE, lenient);
    let mut cheap = o.terms.clone();
    cheap.moderator = lenient_record.clone();
    cheap.network_fee = Amount::from_sat(400);
    let party_ctx = Context {
        moderator: lenient_record.clone(),
        policy: lenient,
        ..ctx()
    };
    let signed = sign_terms(&o, cheap);
    let (b, open) = BuyerSession::start(
        signed.clone(),
        &o.buyer_secret,
        &party_ctx,
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();
    let (v, reply) = VendorSession::accept(signed, &o.vendor_secret, &party_ctx, &open).unwrap();
    let (buyer, sigs) = b.receive(&reply).unwrap();
    v.receive(&sigs).unwrap();
    let mut m = crate::Moderator::new(
        moderator_keys(),
        lenient_record,
        HashMap::from([(outpoint(), buyer.funding_output().clone())]),
        crate::InMemoryRulingStore::default(),
    )
    .unwrap();
    m.open(&buyer.dispute_letter(b"x").unwrap(), TIP).unwrap();
}

#[test]
fn low_network_fee_is_capped_by_policy() {
    let o = order(ORDER_SECRET);
    let mut greedy = o.terms.clone();
    greedy.network_fee = Policy::default().max_network_fee + Amount::from_sat(1);
    assert_eq!(greedy.validate(&Policy::default()), Err(Error::FeeTooHigh));
    assert_eq!(start(&o, greedy, &ctx()), Err(Error::FeeTooHigh));
}

#[test]
fn low_buyer_can_re_sign_the_release_until_it_confirms() {
    let (mut buyer, mut vendor) = funded();
    let lost = vendor.release_nonce().unwrap();
    buyer.sign_release(lost.public()).unwrap();
    assert_eq!(buyer.phase(), Phase::ReleaseSigned);

    let nonce = vendor.release_nonce().unwrap();
    let signature = buyer.sign_release(nonce.public()).unwrap();
    let tx = vendor.complete_release(nonce, &signature).unwrap();
    vendor.verify_key_spend(&tx).unwrap();

    // Once released, the buyer cannot also take the refund or a dispute outcome.
    assert!(matches!(
        buyer.broadcast_refund(REFUND + 1),
        Err(Error::InvalidTransition { .. })
    ));
    let ruling = rule(&buyer.dispute_letter(b"x").unwrap(), TIP).unwrap();
    assert!(matches!(
        buyer.settle_dispute(&ruling),
        Err(Error::InvalidTransition { .. })
    ));
}

#[test]
fn low_moderator_signing_and_encryption_keys_are_separate() {
    let keys = ModeratorKeys::from_seed(&MODERATOR_SEED).unwrap();
    assert_ne!(keys.signing_public(), keys.encryption_public());
    assert_ne!(keys.signing_public(), public(&MODERATOR_SEED));
    let mut same = record();
    same.encryption_key = same.signing_key;
    assert!(matches!(same.validate(), Err(Error::InvalidParams(_))));
    assert_eq!(
        crate::Moderator::new(
            ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED).unwrap(),
            record(),
            HashMap::new(),
            crate::InMemoryRulingStore::default(),
        )
        .err(),
        Some(Error::WrongKey)
    );
}

/// M4: every presigned spend the recipient may need to bump has an anchor.
#[test]
fn m4_presigned_spends_carry_a_pay_to_anchor_output() {
    let (buyer, vendor) = funded();
    let anchored = [
        buyer.refund().transaction().clone(),
        vendor.claim().transaction().clone(),
        buyer.dispute_outcome(Party::Buyer).clone(),
        buyer.dispute_outcome(Party::Vendor).clone(),
    ];
    for tx in &anchored {
        let anchors: Vec<_> = tx
            .output
            .iter()
            .filter(|out| out.script_pubkey == anchor_script())
            .collect();
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].value, ANCHOR_VALUE);
        assert_eq!(
            anchors[0].script_pubkey.as_bytes(),
            [0x51, 0x02, 0x4e, 0x73]
        );
    }
    assert_eq!(
        buyer.refund().transaction().output[0].value,
        FUNDING - NETWORK_FEE - ANCHOR_VALUE
    );
    assert!(Policy::default().min_fee_rate_sat_vb >= 4);
}

/// M6: the moderator caches verified letters, but the cheap checks still run.
#[test]
fn m6_cached_letters_still_get_the_deadline_and_chain_checks() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"a").unwrap();
    let mut m = moderator();
    m.review(&letter, TIP).unwrap();
    m.review(&letter, TIP).unwrap();
    assert_eq!(
        m.review(&letter, DEADLINE + 1).unwrap_err(),
        Error::DisputeWindowClosed
    );
    m.open(&letter, DEADLINE).unwrap();
}
