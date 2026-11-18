use std::collections::HashMap;

use bitcoin::hex::{DisplayHex, FromHex};
use bitcoin::Amount;

use super::support::*;
use crate::tx::Templates;
use crate::{
    Context, Error, EscrowTerms, FeeKind, FundingPlan, InMemoryRulingStore, Moderator,
    ModeratorRecord, Party, Phase, Policy, SignedTerms, ANCHOR_VALUE, MIN_FILING_FEE,
};

const FILING_FEE: Amount = Amount::from_sat(5_000);

// Captured from the v4 implementation before v5 existed. Escrows funded under
// v4 must keep these exact bytes, digest, and transactions.
const V4_TERMS: &str = "4f4245540402da696704e979d445666c9aa1aa9fd631a6de2b4ff77830900de07bf131d7eb4902c587b29f60e975b68472e16742a1ac83738d15f2f70e12fce9676841be0ef4ca035857a455f74fe90d24ff662b01217381b18066644b0aab083a41747630e61e16039c3ca9150985b7d914a525a6829bd79a5c227b7486c85663826e4787b737cd8e220000005120275d3b4a3ec3a27a5220c0ca5dedb676c3d62a0723bd1a75bf53a805730c9ed5d007000000000000040000000000000050c3000000000000f00300009000000090000000a0320000a086010000000000e80300000000000080c30d0010c40d00a0c40d002200000051200f34b06bd6af32a9f25a1a539a96405b1d3dc9cb2b33d9ba8c0fdd52427062b0220000005120c857a78fe23b6a3bd9fb190cb25bf670f716486cde0ff83aae552c1097d761ed";
const V4_DIGEST: &str = "c99b3f88d048523a382e6ba8af5aa4c9838c3012b840c3cfd585d9f4946496c5";
const V4_SIGNED: &str = "4f425453043f0100004f4245540402da696704e979d445666c9aa1aa9fd631a6de2b4ff77830900de07bf131d7eb4902c587b29f60e975b68472e16742a1ac83738d15f2f70e12fce9676841be0ef4ca035857a455f74fe90d24ff662b01217381b18066644b0aab083a41747630e61e16039c3ca9150985b7d914a525a6829bd79a5c227b7486c85663826e4787b737cd8e220000005120275d3b4a3ec3a27a5220c0ca5dedb676c3d62a0723bd1a75bf53a805730c9ed5d007000000000000040000000000000050c3000000000000f00300009000000090000000a0320000a086010000000000e80300000000000080c30d0010c40d00a0c40d002200000051200f34b06bd6af32a9f25a1a539a96405b1d3dc9cb2b33d9ba8c0fdd52427062b0220000005120c857a78fe23b6a3bd9fb190cb25bf670f716486cde0ff83aae552c1097d761edebc6b5fc2ebf69666a6a5f045a65fcd7a93a33153f02a0af7f84e9d27811f04ea988c6a4b0f022d6c3fd512667d68d402fb0529785efbdcb394a1b258ad10c5ce1878e4b6f0db06d304dbe9f26ca90853443426ee618a5862a64c60652e3f0e282d59e5d9217d81d5642269a7ff1cb76a4da2bcec9f0f5ace80a08df326899fb";
const V4_RECORD: &str = "4f424d5201035857a455f74fe90d24ff662b01217381b18066644b0aab083a41747630e61e16039c3ca9150985b7d914a525a6829bd79a5c227b7486c85663826e4787b737cd8e220000005120275d3b4a3ec3a27a5220c0ca5dedb676c3d62a0723bd1a75bf53a805730c9ed5d007000000000000040000000000000050c3000000000000f00300009000000090000000a0320000";
const V4_TXIDS: [&str; 5] = [
    "4b98d17684d047acae66cf4c81723508c23a3afd87bd66f6ea9fa89702fd2aaf",
    "f848a42a8a315be94b1e34b40ebb0abe653366099174d1f8e9c7ad8428c1f7bc",
    "f848a42a8a315be94b1e34b40ebb0abe653366099174d1f8e9c7ad8428c1f7bc",
    "0aa891b9bc2e118de5ce3f5a6591ed33de5eb01eea37eaa251d6294ee0f4f7f6",
    "144c596d7fb460cdbb406322a9ce1523d80966b04a13af8eeb4d55d31761d08e",
];

fn bytes(hex: &str) -> Vec<u8> {
    Vec::from_hex(hex).unwrap()
}

fn filing_record(fee: Amount) -> ModeratorRecord {
    moderator_keys().filing_record(fee, Policy::default())
}

fn filing_ctx() -> Context {
    Context {
        moderator: filing_record(FILING_FEE),
        ..ctx()
    }
}

fn filing_order() -> Order {
    let mut o = order(ORDER_SECRET);
    o.terms.moderator = filing_record(FILING_FEE);
    o
}

fn filing_moderator(
    terms: &EscrowTerms,
) -> Moderator<HashMap<bitcoin::OutPoint, bitcoin::TxOut>, InMemoryRulingStore> {
    let chain = HashMap::from([(outpoint(), terms.funding_output().unwrap())]);
    Moderator::new(
        moderator_keys(),
        filing_record(FILING_FEE),
        chain,
        InMemoryRulingStore::default(),
    )
    .unwrap()
}

#[test]
fn v4_terms_keep_their_exact_bytes_digest_and_transactions() {
    let terms = order(ORDER_SECRET).terms;
    assert_eq!(terms.version(), 4);
    assert_eq!(terms.encode().to_lower_hex_string(), V4_TERMS);
    assert_eq!(terms.digest().to_lower_hex_string(), V4_DIGEST);
    assert_eq!(EscrowTerms::decode(&bytes(V4_TERMS)).unwrap(), terms);
    assert_eq!(record().to_bytes().to_lower_hex_string(), V4_RECORD);
    assert_eq!(
        ModeratorRecord::from_bytes(&bytes(V4_RECORD)).unwrap(),
        record()
    );

    let signed = SignedTerms::decode(&bytes(V4_SIGNED)).unwrap();
    assert_eq!(signed.terms(), &terms);
    assert_eq!(signed.encode().to_lower_hex_string(), V4_SIGNED);

    let templates = Templates::build(&terms, outpoint()).unwrap();
    let txids: Vec<_> = templates
        .all()
        .iter()
        .map(|tx| tx.compute_txid().to_string())
        .collect();
    assert_eq!(txids, V4_TXIDS);
}

#[test]
fn a_filing_record_round_trips_without_a_payout_script() {
    let record = filing_record(FILING_FEE);
    assert_eq!(record.fee_kind, FeeKind::Filing);
    assert_eq!(record.filing_fee(), Some(FILING_FEE));
    let encoded = record.to_bytes();
    assert_eq!(&encoded[..5], b"OBMR\x02");
    assert_eq!(ModeratorRecord::from_bytes(&encoded).unwrap(), record);
    assert_eq!(super::support::record().filing_fee(), None);
}

#[test]
fn a_filing_fee_is_zero_or_above_the_p2tr_dust_limit() {
    filing_record(Amount::ZERO).validate().unwrap();
    filing_record(MIN_FILING_FEE).validate().unwrap();
    assert_eq!(
        filing_record(MIN_FILING_FEE - Amount::from_sat(1)).validate(),
        Err(Error::NonStandardOutput)
    );
    let mut with_payout = filing_record(FILING_FEE);
    with_payout.payout = p2tr(0xb3);
    assert!(matches!(
        with_payout.validate(),
        Err(Error::InvalidParams(_))
    ));
}

#[test]
fn record_versions_cannot_be_swapped() {
    let mut v1_as_v2 = record().to_bytes();
    v1_as_v2[4] = 2;
    assert!(ModeratorRecord::from_bytes(&v1_as_v2).is_err());
    let mut v2_as_v1 = filing_record(FILING_FEE).to_bytes();
    v2_as_v1[4] = 1;
    assert!(ModeratorRecord::from_bytes(&v2_as_v1).is_err());
    let mut v3 = filing_record(FILING_FEE).to_bytes();
    v3[4] = 3;
    assert_eq!(ModeratorRecord::from_bytes(&v3), Err(Error::Malformed));
}

#[test]
fn v5_terms_round_trip_under_their_own_digest() {
    let o = filing_order();
    o.terms.validate(&Policy::default()).unwrap();
    assert_eq!(o.terms.version(), 5);
    let encoded = o.terms.encode();
    assert_eq!(&encoded[..5], b"OBET\x05");
    assert_eq!(EscrowTerms::decode(&encoded).unwrap(), o.terms);

    let signed = sign(&o);
    let signed_bytes = signed.encode();
    assert_eq!(&signed_bytes[..5], b"OBTS\x05");
    assert_eq!(SignedTerms::decode(&signed_bytes).unwrap(), signed);

    // Same keys, timeline, and payouts under a v1 record: a different digest,
    // so a v4 signature never verifies as v5.
    let v4 = order(ORDER_SECRET).terms;
    assert_ne!(v4.digest(), o.terms.digest());
    let v4_signatures = (
        v4.sign(&o.buyer_secret).unwrap(),
        v4.sign(&o.vendor_secret).unwrap(),
    );
    assert_eq!(
        SignedTerms::new(o.terms.clone(), v4_signatures.0, v4_signatures.1).unwrap_err(),
        Error::BadTermsSignature
    );
}

#[test]
fn signed_wrapper_version_must_match_its_terms() {
    let signed = sign(&filing_order()).encode();
    let mut as_v4 = signed.clone();
    as_v4[4] = 4;
    assert_eq!(SignedTerms::decode(&as_v4).unwrap_err(), Error::Malformed);

    let v4 = sign(&order(ORDER_SECRET)).encode();
    let mut as_v5 = v4.clone();
    as_v5[4] = 5;
    assert_eq!(SignedTerms::decode(&as_v5).unwrap_err(), Error::Malformed);
}

#[test]
fn v5_dispute_outcomes_pay_the_winner_and_no_moderator() {
    let o = filing_order();
    let templates = Templates::build(&o.terms, outpoint()).unwrap();
    let anchored = FUNDING - NETWORK_FEE - ANCHOR_VALUE;
    for (tx, party) in [
        (&templates.buyer_wins, Party::Buyer),
        (&templates.vendor_wins, Party::Vendor),
    ] {
        assert_eq!(tx.output.len(), 2);
        assert_eq!(&tx.output[0].script_pubkey, o.terms.payout(party));
        assert_eq!(tx.output[0].value, anchored);
        assert_eq!(tx.output[1].value, ANCHOR_VALUE);
    }
    assert_eq!(templates.refund.output[0].value, anchored);
    assert_eq!(templates.release.output[0].value, FUNDING - NETWORK_FEE);
}

#[test]
fn a_v5_fee_larger_than_the_escrow_does_not_shrink_any_payout() {
    let mut o = filing_order();
    o.terms.moderator = filing_record(FUNDING * 2);
    o.terms.validate(&Policy::default()).unwrap();
    let templates = Templates::build(&o.terms, outpoint()).unwrap();
    assert_eq!(
        templates.buyer_wins.output[0].value,
        FUNDING - NETWORK_FEE - ANCHOR_VALUE
    );
}

#[test]
fn a_v5_escrow_disputes_and_settles_end_to_end() {
    let o = filing_order();
    let (mut buyer, mut vendor) = run_funding_in(
        &filing_ctx(),
        &o,
        sign(&o),
        FundingPlan::Outpoint(outpoint()),
    )
    .unwrap();

    let letter = buyer.dispute_letter(b"never shipped").unwrap();
    let ruling = filing_moderator(&o.terms).open(&letter, DEADLINE).unwrap();
    let won = buyer.settle_dispute(&ruling).unwrap();
    assert_eq!(buyer.phase(), Phase::Disputed);
    buyer.verify_key_spend(&won).unwrap();
    assert_eq!(won.output.len(), 2);
    assert_eq!(won.output[0].value, FUNDING - NETWORK_FEE - ANCHOR_VALUE);

    let letter = vendor.dispute_letter(b"delivered").unwrap();
    let ruling = filing_moderator(&o.terms).open(&letter, DEADLINE).unwrap();
    let won = vendor.settle_dispute(&ruling).unwrap();
    vendor.verify_key_spend(&won).unwrap();
    assert_eq!(won.output[0].script_pubkey, o.terms.vendor_payout);
}

#[test]
fn a_v5_moderator_does_not_review_v4_letters_under_its_v2_record() {
    let (buyer, _) = funded();
    let letter = buyer.dispute_letter(b"memo").unwrap();
    let v4_terms = order(ORDER_SECRET).terms;
    assert_eq!(
        filing_moderator(&v4_terms)
            .open(&letter, DEADLINE)
            .unwrap_err(),
        Error::ModeratorMismatch
    );
    // The same keys under the v1 record still rule for it.
    rule(&letter, DEADLINE).unwrap();
}

#[test]
fn parties_reject_terms_naming_the_other_fee_kind() {
    let o = filing_order();
    assert_eq!(
        run_funding_in(&ctx(), &o, sign(&o), FundingPlan::Outpoint(outpoint())).unwrap_err(),
        Error::ModeratorMismatch
    );
    let v4 = order(ORDER_SECRET);
    assert_eq!(
        run_funding_in(
            &filing_ctx(),
            &v4,
            sign(&v4),
            FundingPlan::Outpoint(outpoint())
        )
        .unwrap_err(),
        Error::ModeratorMismatch
    );
}
