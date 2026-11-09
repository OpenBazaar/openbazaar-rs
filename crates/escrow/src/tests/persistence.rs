use std::cell::Cell;

use super::support::*;
use crate::{
    BuyerEscrow, BuyerOpen, BuyerSession, BuyerSignatures, Context, Error, FundingPlan, Party,
    Phase, ReleaseSignature, VendorAwaitingSignatures, VendorEscrow, VendorReply, VendorSession,
};

/// Long after funding: a restore must not re-check the funding-time heights.
fn later() -> Context {
    Context {
        tip: CLAIM + 10,
        ..ctx()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn funded_states_restore_and_still_release_and_dispute() {
    let order = order(ORDER_SECRET);
    let (buyer, vendor) = funded();
    let buyer_bytes = buyer.snapshot();
    let vendor_bytes = vendor.snapshot();
    assert!(!contains(&buyer_bytes, &order.buyer_secret));
    assert!(!contains(&vendor_bytes, &order.vendor_secret));

    let mut buyer = BuyerEscrow::restore(&buyer_bytes, &order.buyer_secret, &later()).unwrap();
    let mut vendor = VendorEscrow::restore(&vendor_bytes, &order.vendor_secret, &later()).unwrap();
    assert_eq!(buyer.phase(), Phase::Funded);
    assert_eq!(buyer.snapshot(), buyer_bytes, "a restore is byte-for-byte stable");
    assert_eq!(vendor.snapshot(), vendor_bytes);

    let nonce = vendor.release_nonce().unwrap();
    let signature = buyer.sign_release(nonce.public()).unwrap();
    let signature = ReleaseSignature::from_bytes(&signature.to_bytes()).unwrap();
    let release = vendor.complete_release(nonce, &signature).unwrap();
    vendor.verify_key_spend(&release).unwrap();

    // The phase survives: a restored buyer that signed the release cannot refund.
    let signed = BuyerEscrow::restore(&buyer.snapshot(), &order.buyer_secret, &later()).unwrap();
    assert_eq!(signed.phase(), Phase::ReleaseSigned);
    let mut signed = signed;
    assert!(matches!(
        signed.broadcast_refund(REFUND + 1),
        Err(Error::InvalidTransition { .. })
    ));

    let (_, vendor) = funded();
    let mut vendor = VendorEscrow::restore(&vendor.snapshot(), &order.vendor_secret, &ctx()).unwrap();
    let ruling = rule(&vendor.dispute_letter(b"delivered").unwrap(), TIP).unwrap();
    let won = vendor.settle_dispute(&ruling).unwrap();
    vendor.verify_key_spend(&won).unwrap();
    let settled = VendorEscrow::restore(&vendor.snapshot(), &order.vendor_secret, &ctx()).unwrap();
    assert_eq!(settled.phase(), Phase::Disputed);
}

#[test]
fn a_restore_needs_the_right_secret_and_untouched_bytes() {
    let order = order(ORDER_SECRET);
    let (buyer, vendor) = funded();
    let bytes = buyer.snapshot();
    assert_eq!(
        BuyerEscrow::restore(&bytes, &order.vendor_secret, &ctx()).unwrap_err(),
        Error::WrongKey
    );
    assert!(VendorEscrow::restore(&bytes, &order.vendor_secret, &ctx()).is_err());
    assert!(BuyerEscrow::restore(&vendor.snapshot(), &order.buyer_secret, &ctx()).is_err());
    let other = Context {
        moderator: other_record(),
        ..ctx()
    };
    assert_eq!(
        BuyerEscrow::restore(&bytes, &order.buyer_secret, &other).unwrap_err(),
        Error::ModeratorMismatch
    );
    // Flip bytes across the snapshot outside the (unproved) share.
    let step = bytes.len() / 61;
    let mut refused = 0;
    for at in (0..bytes.len()).step_by(step.max(1)) {
        let mut tampered = bytes.clone();
        tampered[at] ^= 0x01;
        if BuyerEscrow::restore(&tampered, &order.buyer_secret, &ctx()).is_err() {
            refused += 1;
        }
    }
    assert!(refused > 0);
    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        BuyerEscrow::restore(&truncated, &order.buyer_secret, &ctx()).unwrap_err(),
        Error::Malformed
    );
}

#[test]
fn a_vendor_restarted_after_round_two_still_finishes_funding() {
    let order = order(ORDER_SECRET);
    let signed = sign(&order);
    let ctx = ctx();
    let (buyer, open) = BuyerSession::start(
        signed.clone(),
        &order.buyer_secret,
        &ctx,
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();
    let open = BuyerOpen::from_bytes(&open.to_bytes()).unwrap();
    let terms_len = signed.encode().len();
    let (vendor, reply) = VendorSession::accept(signed, &order.vendor_secret, &ctx, &open).unwrap();
    let stored = vendor.snapshot();
    drop(vendor);
    assert!(!contains(&stored, &order.vendor_secret));
    assert!(VendorAwaitingSignatures::restore(&stored, &order.buyer_secret, &ctx).is_err());

    let reply = VendorReply::from_bytes(&reply.to_bytes()).unwrap();
    let (_, signatures) = buyer.receive(&reply).unwrap();
    let signatures = BuyerSignatures::from_bytes(&signatures.to_bytes()).unwrap();
    let vendor = VendorAwaitingSignatures::restore(&stored, &order.vendor_secret, &later()).unwrap();
    assert_eq!(vendor.funding_outpoint(), outpoint());
    let vendor = vendor.receive(&signatures).unwrap();
    assert_eq!(vendor.claim().transaction().lock_time.to_consensus_u32(), REFUND);

    // A stored claim partial that is not the vendor's own is refused.
    let mut forged = stored.clone();
    let claim_partial = 5 + 4 + terms_len + 36 + 2 * 4 * 66;
    let at = claim_partial + 7;
    forged[at] ^= 0x01;
    assert!(VendorAwaitingSignatures::restore(&forged, &order.vendor_secret, &ctx).is_err());
}

#[test]
fn m6_the_gate_runs_before_proofs_and_only_for_unverified_letters() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"rate me").unwrap();
    let mut first = moderator();
    let calls = Cell::new(0);
    let refuse = &mut |_: &crate::RulingKey, favored: Party| {
        calls.set(calls.get() + 1);
        assert_eq!(favored, Party::Buyer);
        Err(Error::RateLimited)
    };
    assert_eq!(
        first.review_gated(&letter, TIP, refuse).unwrap_err(),
        Error::RateLimited
    );
    assert_eq!(calls.get(), 1);

    // A letter refused by a local check never reaches the gate.
    let mut forged = letter.clone();
    let last = forged.len() - 1;
    forged[last] ^= 0x01;
    assert_eq!(
        first.review_gated(&forged, TIP, refuse).unwrap_err(),
        Error::BadSignature
    );
    assert_eq!(calls.get(), 1);

    // The gate runs before the chain lookup, so a refused letter costs no scan.
    let mut unfunded = moderator_with_chain(Default::default());
    assert_eq!(
        unfunded.review_gated(&letter, TIP, refuse).unwrap_err(),
        Error::RateLimited
    );
    assert_eq!(calls.get(), 2);
    assert_eq!(
        unfunded.review(&letter, TIP).unwrap_err(),
        Error::FundingNotOnChain
    );

    // Once the proofs verified, the same bytes skip the gate and the proofs.
    let cache = std::sync::Arc::new(crate::LetterCache::default());
    moderator().with_cache(cache.clone()).review(&letter, TIP).unwrap();
    moderator()
        .with_cache(cache)
        .review_gated(&letter, TIP, refuse)
        .unwrap();
    assert_eq!(calls.get(), 2);
}

#[test]
fn max_spend_vsize_covers_every_presigned_spend() {
    let terms = order(ORDER_SECRET).terms;
    let vsize = crate::max_spend_vsize(&terms).unwrap();
    let (buyer, vendor) = funded();
    for tx in [
        buyer.refund().transaction(),
        vendor.claim().transaction(),
        buyer.dispute_outcome(Party::Buyer),
        buyer.dispute_outcome(Party::Vendor),
    ] {
        let signed = crate::tx::attach_key_spend(tx.clone(), &[0u8; 64]);
        assert!(signed.vsize() as u64 <= vsize);
    }
    assert!(vsize >= 150 && vsize < 200, "{vsize}");
}
