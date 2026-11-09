use bitcoin::hashes::Hash;
use bitcoin::psbt::Psbt;
use bitcoin::{
    absolute, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};

use super::support::*;
use crate::{
    order_public_key, BuyerOpen, BuyerSession, Context, Error, FundingPlan, Policy, SignedTerms,
    VendorReply, VendorSession,
};

#[test]
fn two_parties_fund_by_exchanging_only_messages() {
    let (buyer, vendor) = funded();
    assert_eq!(buyer.output_key(), vendor.output_key());
    assert_eq!(buyer.funding_output(), vendor.funding_output());
    assert_eq!(buyer.funding_outpoint(), outpoint());
    assert_eq!(
        buyer.funding_output(),
        &order(ORDER_SECRET).terms.funding_output().unwrap()
    );

    let refund = buyer.refund().transaction();
    buyer.verify_key_spend(refund).unwrap();
    vendor.verify_key_spend(refund).unwrap();
    assert_eq!(refund.lock_time.to_consensus_u32(), REFUND);
    assert_eq!(refund.input[0].witness.nth(0).unwrap().len(), 64);

    let claim = vendor.claim().transaction();
    buyer.verify_key_spend(claim).unwrap();
    assert_eq!(claim.lock_time.to_consensus_u32(), REFUND);
    assert_eq!(
        claim.output[0].script_pubkey,
        order(ORDER_SECRET).terms.buyer_payout
    );
    assert_ne!(
        claim.output[0].script_pubkey,
        order(ORDER_SECRET).terms.vendor_payout
    );
}

#[test]
fn vendor_rejects_a_garbage_vendor_wins_share() {
    let o = order(ORDER_SECRET);
    let ctx = ctx();
    let (_, open) = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &ctx,
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();

    let mut garbage = open.clone();
    garbage.vendor_wins_share.corrupt_bit(17);
    let accept =
        |msg: &BuyerOpen| VendorSession::accept(sign(&o), &o.vendor_secret, &ctx, msg).map(|_| ());
    assert_eq!(accept(&garbage), Err(Error::BadProof));

    let mut swapped = open.clone();
    swapped.vendor_wins_share = open_for_other_moderator(&o).vendor_wins_share;
    assert_eq!(accept(&swapped), Err(Error::BadProof));

    let mut wrong_pok = open.clone();
    wrong_pok.buyer_half = open.vendor_wins_share.point();
    assert_eq!(accept(&wrong_pok), Err(Error::BadProof));

    accept(&open).unwrap();
}

fn open_for_other_moderator(o: &Order) -> BuyerOpen {
    let mut terms = o.terms.clone();
    terms.moderator = other_record();
    let ctx = Context {
        moderator: other_record(),
        ..ctx()
    };
    BuyerSession::start(
        sign_terms(o, terms),
        &o.buyer_secret,
        &ctx,
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap()
    .1
}

#[test]
fn buyer_rejects_a_garbage_buyer_wins_share_and_never_sees_the_psbt() {
    let o = order(ORDER_SECRET);
    let ctx = ctx();
    let start = || {
        BuyerSession::start(
            sign(&o),
            &o.buyer_secret,
            &ctx,
            FundingPlan::Psbt(segwit_psbt(&o, segwit_input())),
        )
        .unwrap()
    };
    let reply_for = |open: &BuyerOpen| {
        VendorSession::accept(sign(&o), &o.vendor_secret, &ctx, open)
            .unwrap()
            .1
    };

    let (buyer, open) = start();
    let mut reply = reply_for(&open);
    reply.buyer_wins_share.corrupt_bit(200);
    assert_eq!(buyer.receive(&reply).unwrap_err(), Error::BadProof);

    let (buyer, open) = start();
    let mut reply = reply_for(&open);
    reply.vendor_half = reply.buyer_wins_share.point();
    assert_eq!(buyer.receive(&reply).unwrap_err(), Error::BadProof);
}

#[test]
fn funding_psbt_is_only_released_after_a_verified_refund() {
    let o = order(ORDER_SECRET);
    let ctx = ctx();
    let psbt = segwit_psbt(&o, segwit_input());
    let txid = psbt.unsigned_tx.compute_txid();

    let (buyer, open) = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &ctx,
        FundingPlan::Psbt(psbt.clone()),
    )
    .unwrap();
    assert_eq!(buyer.funding_outpoint(), OutPoint { txid, vout: 0 });
    let (_, reply) = VendorSession::accept(sign(&o), &o.vendor_secret, &ctx, &open).unwrap();

    // A vendor who sends a bad refund partial leaves the buyer with nothing to
    // fund: `receive` consumes the state, PSBT included, and returns an error.
    // (`BuyerAwaitingReply` has no PSBT accessor at all; see the doctest on it.)
    let (stalled, open2) =
        BuyerSession::start(sign(&o), &o.buyer_secret, &ctx, FundingPlan::Psbt(psbt)).unwrap();
    let (_, reply2) = VendorSession::accept(sign(&o), &o.vendor_secret, &ctx, &open2).unwrap();
    let mut bad: VendorReply = reply2.clone();
    bad.refund_partial = reply2.buyer_wins_partial;
    assert_eq!(stalled.receive(&bad).unwrap_err(), Error::BadPartial);

    let (escrow, _) = buyer.receive(&reply).unwrap();
    let released = escrow.funding_psbt().expect("PSBT handed back");
    assert_eq!(released.unsigned_tx.compute_txid(), txid);
    let refund = escrow.refund().transaction();
    assert_eq!(refund.input[0].previous_output, OutPoint { txid, vout: 0 });
    escrow.verify_key_spend(refund).unwrap();
}

fn start_with_psbt(o: &Order, psbt: Psbt) -> Result<(), Error> {
    BuyerSession::start(sign(o), &o.buyer_secret, &ctx(), FundingPlan::Psbt(psbt)).map(|_| ())
}

#[test]
fn non_segwit_or_mismatched_funding_psbts_are_rejected() {
    let o = order(ORDER_SECRET);
    let start = |psbt: Psbt| start_with_psbt(&o, psbt);
    let legacy = TxOut {
        value: Amount::from_sat(200_000),
        script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::from_byte_array([7; 20])),
    };
    assert!(matches!(
        start(segwit_psbt(&o, legacy)),
        Err(Error::BadFunding(_))
    ));

    let mut missing = segwit_psbt(&o, segwit_input());
    missing.inputs[0].witness_utxo = None;
    assert!(matches!(start(missing), Err(Error::BadFunding(_))));

    let mut wrapped = segwit_psbt(&o, segwit_input());
    wrapped.inputs[0].redeem_script = Some(ScriptBuf::new());
    assert!(matches!(start(wrapped), Err(Error::BadFunding(_))));

    let mut underpaid = segwit_psbt(&o, segwit_input());
    underpaid.unsigned_tx.output[0].value = FUNDING - Amount::from_sat(1);
    assert!(matches!(start(underpaid), Err(Error::BadFunding(_))));
}

#[test]
fn funding_psbts_that_signal_rbf_are_rejected() {
    let o = order(ORDER_SECRET);
    let mut replaceable = segwit_psbt(&o, segwit_input());
    replaceable.unsigned_tx.input[0].sequence = Sequence::ENABLE_RBF_NO_LOCKTIME;
    assert!(matches!(
        start_with_psbt(&o, replaceable),
        Err(Error::BadFunding(_))
    ));
    let mut locktime_only = segwit_psbt(&o, segwit_input());
    locktime_only.unsigned_tx.input[0].sequence = Sequence::ENABLE_LOCKTIME_NO_RBF;
    start_with_psbt(&o, locktime_only).unwrap();
}

#[test]
fn a_moderator_outside_the_signed_terms_is_rejected() {
    let o = order(ORDER_SECRET);
    let other = Context {
        moderator: other_record(),
        ..ctx()
    };
    let err = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &other,
        FundingPlan::Outpoint(outpoint()),
    )
    .map(|_| ())
    .unwrap_err();
    assert_eq!(err, Error::ModeratorMismatch);

    let (_, open) = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &ctx(),
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();
    let err = VendorSession::accept(sign(&o), &o.vendor_secret, &other, &open)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err, Error::ModeratorMismatch);

    // Swapping the moderator, fee, payout, or policy after signing breaks the terms signatures.
    let buyer_sig = o.terms.sign(&o.buyer_secret).unwrap();
    let vendor_sig = o.terms.sign(&o.vendor_secret).unwrap();
    let edits: [fn(&mut crate::EscrowTerms); 4] = [
        |t| t.moderator = other_record(),
        |t| t.moderator.fee = Amount::from_sat(1_000),
        |t| t.moderator.payout = p2tr(0xb4),
        |t| t.moderator.policy.min_ruling_window += 1,
    ];
    for edit in edits {
        let mut edited = o.terms.clone();
        edit(&mut edited);
        assert_eq!(
            SignedTerms::new(edited, buyer_sig, vendor_sig).unwrap_err(),
            Error::BadTermsSignature
        );
    }
    assert_eq!(
        SignedTerms::new(o.terms.clone(), buyer_sig, buyer_sig).unwrap_err(),
        Error::BadTermsSignature
    );
}

#[test]
fn each_party_session_takes_only_its_own_key() {
    let o = order(ORDER_SECRET);
    let err = BuyerSession::start(
        sign(&o),
        &o.vendor_secret,
        &ctx(),
        FundingPlan::Outpoint(outpoint()),
    )
    .map(|_| ())
    .unwrap_err();
    assert_eq!(err, Error::WrongKey);
    assert_eq!(o.terms.sign(&MODERATOR_SEED).unwrap_err(), Error::WrongKey);
}

#[test]
fn per_order_keys_make_orders_between_the_same_pair_unlinkable() {
    let first = order([0x01; 32]);
    let second = order([0x02; 32]);
    let first_script = first.terms.funding_output().unwrap().script_pubkey;
    let second_script = second.terms.funding_output().unwrap().script_pubkey;
    assert_ne!(first_script, second_script);
    assert_ne!(first.terms.buyer, second.terms.buyer);
    assert_ne!(first.terms.vendor, second.terms.vendor);

    // The long-term keys alone, or another order's secret, give neither key.
    let buyer_long = public(&BUYER_LONG_TERM);
    assert_ne!(buyer_long, first.terms.buyer);
    assert_ne!(
        order_public_key(&buyer_long, &[0x03; 32]).unwrap(),
        first.terms.buyer
    );
    assert_eq!(
        order_public_key(&buyer_long, &[0x01; 32]).unwrap(),
        first.terms.buyer
    );

    // Both orders still fund independently.
    let (a, _) = run_funding(&first, FundingPlan::Outpoint(outpoint())).unwrap();
    assert_eq!(a.funding_output().script_pubkey, first_script);
}

#[test]
fn presigned_spends_need_a_real_fee_rate() {
    let o = order(ORDER_SECRET);
    let policy = Policy::default();

    let mut zero = o.terms.clone();
    zero.network_fee = Amount::ZERO;
    assert_eq!(zero.validate(&policy), Err(Error::FeeTooLow));

    // The dispute outcomes are the largest presigned spends (three outputs).
    let dispute_vsize = {
        let tx = crate::tx::attach_key_spend(
            crate::tx::Templates::build(&o.terms, outpoint())
                .unwrap()
                .buyer_wins,
            &[0; 64],
        );
        tx.vsize() as u64
    };
    let floor = dispute_vsize * policy.min_fee_rate_sat_vb;
    let mut thin = o.terms.clone();
    thin.network_fee = Amount::from_sat(floor - 1);
    assert_eq!(thin.validate(&policy), Err(Error::FeeTooLow));
    thin.network_fee = Amount::from_sat(floor);
    thin.validate(&policy).unwrap();

    let strict = Policy {
        min_fee_rate_sat_vb: 10,
        ..policy
    };
    assert_eq!(o.terms.validate(&strict), Err(Error::FeeTooLow));
    let err = BuyerSession::start(
        sign(&o),
        &o.buyer_secret,
        &Context {
            policy: strict,
            ..ctx()
        },
        FundingPlan::Outpoint(outpoint()),
    )
    .map(|_| ())
    .unwrap_err();
    assert_eq!(err, Error::FeeTooLow);
}

#[test]
fn timeline_must_order_dispute_claim_refund() {
    let o = order(ORDER_SECRET);
    let policy = Policy::default();
    let bad = |edit: fn(&mut crate::EscrowTerms)| {
        let mut terms = o.terms.clone();
        edit(&mut terms);
        terms.validate(&policy)
    };
    assert!(bad(|t| t.vendor_claim_height = t.dispute_deadline).is_err());
    assert!(bad(|t| t.refund_height = t.vendor_claim_height).is_err());
    assert!(bad(|t| t.refund_height = t.vendor_claim_height - 1).is_err());
    assert!(bad(|t| t.vendor_claim_height = t.dispute_deadline + 143).is_err());
    assert!(bad(|t| t.dispute_deadline = 0).is_err());
    o.terms.validate(&policy).unwrap();
}

pub(super) fn segwit_input() -> TxOut {
    TxOut {
        value: Amount::from_sat(200_000),
        script_pubkey: ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([9; 20])),
    }
}

pub(super) fn segwit_psbt(o: &Order, input: TxOut) -> Psbt {
    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0x77; 32]),
                vout: 3,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![
            o.terms.funding_output().unwrap(),
            TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: p2tr(0xcc),
            },
        ],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
    psbt.inputs[0].witness_utxo = Some(input);
    psbt
}
