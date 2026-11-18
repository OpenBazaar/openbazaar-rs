use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, Txid};
use openbazaar_protocol::{FeePayment, FeeReceipt, FeeRequest, FAVORED_BUYER, FAVORED_VENDOR};

use super::support::*;
use crate::{
    fee_letter_digest, is_denial_reversed, read_case_accepted, read_dispute_decision, read_fee_receipt,
    read_fee_request, Decision, Error, EscrowTerms, FeeCase, ModeratorKeys, Party, Policy, RulingKey,
};

const FEE: Amount = Amount::from_sat(5_000);
const LETTER: &[u8] = b"the letter bytes";

fn v5_terms(fee: Amount) -> EscrowTerms {
    let mut terms = order(ORDER_SECRET).terms;
    terms.moderator = moderator_keys().filing_record(fee, Policy::default());
    terms
}

fn case(terms: &EscrowTerms) -> FeeCase<'_> {
    FeeCase {
        terms,
        outpoint: outpoint(),
        filer: Party::Vendor,
        letter_digest: fee_letter_digest(LETTER),
    }
}

fn request(terms: &EscrowTerms) -> FeeRequest {
    FeeRequest {
        moderator_key: moderator_keys().record_key().unwrap(),
        case_id: case(terms).case_id(),
        letter_digest: fee_letter_digest(LETTER),
        favored: FAVORED_VENDOR,
        amount_sats: FEE.to_sat(),
        invoice: None,
        script: p2tr(0xc1).to_bytes(),
        valid_until_height: DEADLINE,
        due_height: CLAIM,
    }
}

fn signed(request: FeeRequest) -> Vec<u8> {
    moderator_keys().sign_fee_request(request).unwrap()
}

#[test]
fn the_case_id_names_one_escrow() {
    let terms = v5_terms(FEE);
    let id = case(&terms).case_id();
    let moved = RulingKey {
        terms_digest: terms.digest(),
        outpoint: OutPoint { vout: 2, ..outpoint() },
    };
    assert_ne!(moved.case_id(), id);
    let other_txid = RulingKey {
        terms_digest: terms.digest(),
        outpoint: OutPoint {
            txid: Txid::from_byte_array([0xac; 32]),
            vout: 1,
        },
    };
    assert_ne!(other_txid.case_id(), id);
    assert_ne!(case(&v5_terms(FEE + Amount::from_sat(1))).case_id(), id);
}

#[test]
fn a_filer_accepts_only_the_request_its_terms_allow() {
    let terms = v5_terms(FEE);
    let (read, digest) = read_fee_request(&signed(request(&terms)), &case(&terms)).unwrap();
    assert_eq!(read, request(&terms));
    assert_ne!(digest, [0; 32]);

    let refused = [
        FeeRequest {
            amount_sats: FEE.to_sat() + 1,
            ..request(&terms)
        },
        FeeRequest {
            amount_sats: FEE.to_sat() - 1,
            ..request(&terms)
        },
        FeeRequest {
            case_id: [9; 32],
            ..request(&terms)
        },
        FeeRequest {
            letter_digest: [9; 32],
            ..request(&terms)
        },
        FeeRequest {
            favored: FAVORED_BUYER,
            ..request(&terms)
        },
        FeeRequest {
            script: vec![0x6a, 0x01, 0x00],
            ..request(&terms)
        },
        FeeRequest {
            due_height: CLAIM + 1,
            valid_until_height: CLAIM - 1,
            ..request(&terms)
        },
    ];
    for bad in refused {
        assert!(
            read_fee_request(&signed(bad.clone()), &case(&terms)).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn a_request_from_another_key_is_refused() {
    let terms = v5_terms(FEE);
    let other = ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED).unwrap();
    let forged = other
        .sign_fee_request(FeeRequest {
            moderator_key: other.record_key().unwrap(),
            ..request(&terms)
        })
        .unwrap();
    assert_eq!(
        read_fee_request(&forged, &case(&terms)).unwrap_err(),
        Error::ModeratorMismatch
    );
}

#[test]
fn no_request_is_payable_under_v4_terms_or_a_zero_fee() {
    let v4 = order(ORDER_SECRET).terms;
    let bytes = signed(FeeRequest {
        case_id: case(&v4).case_id(),
        ..request(&v4)
    });
    assert_eq!(read_fee_request(&bytes, &case(&v4)).unwrap_err(), Error::BadPayload);
    let free = v5_terms(Amount::ZERO);
    let bytes = signed(FeeRequest {
        case_id: case(&free).case_id(),
        ..request(&free)
    });
    assert_eq!(read_fee_request(&bytes, &case(&free)).unwrap_err(), Error::BadPayload);
}

#[test]
fn a_receipt_cites_the_paid_request_and_the_case() {
    let terms = v5_terms(FEE);
    let (_, digest) = read_fee_request(&signed(request(&terms)), &case(&terms)).unwrap();
    let receipt = FeeReceipt {
        moderator_key: moderator_keys().record_key().unwrap(),
        case_id: case(&terms).case_id(),
        request_digest: digest,
        payment: FeePayment::Lightning { payment_hash: [3; 32] },
        received_sats: FEE.to_sat(),
        paid_height: TIP + 1,
        due_height: CLAIM,
    };
    let bytes = moderator_keys().sign_fee_receipt(receipt.clone()).unwrap();
    assert_eq!(read_fee_receipt(&bytes, &case(&terms), &digest).unwrap(), receipt);
    assert_eq!(
        read_fee_receipt(&bytes, &case(&terms), &[7; 32]).unwrap_err(),
        Error::BadPayload
    );
    let other = v5_terms(FEE + Amount::from_sat(1));
    assert_eq!(
        read_fee_receipt(&bytes, &case(&other), &digest).unwrap_err(),
        Error::BadPayload
    );
}

#[test]
fn decisions_cite_their_ruling_and_a_reversed_denial_is_provable() {
    let (buyer, vendor) = funded();
    let key = RulingKey {
        terms_digest: buyer.terms().digest(),
        outpoint: outpoint(),
    };
    let case_id = key.case_id();
    let signing_key = moderator_keys().signing_public();
    let keys = moderator_keys();

    let ruling = rule(&vendor.dispute_letter(b"delivered").unwrap(), DEADLINE)
        .unwrap()
        .to_bytes();
    let ruled = keys
        .sign_dispute_decision(case_id, Decision::Ruled(Party::Vendor), Some(&ruling))
        .unwrap();
    let (decision, record) = read_dispute_decision(&ruled, &signing_key).unwrap();
    assert_eq!(decision, Decision::Ruled(Party::Vendor));
    assert_eq!(record.ruling_digest, openbazaar_protocol::ruling_digest(&ruling));
    assert!(keys
        .sign_dispute_decision(case_id, Decision::Ruled(Party::Buyer), Some(&ruling))
        .is_err());
    assert!(keys
        .sign_dispute_decision([9; 32], Decision::Ruled(Party::Vendor), Some(&ruling))
        .is_err());
    assert!(keys
        .sign_dispute_decision(case_id, Decision::Denied(Party::Vendor), Some(&ruling))
        .is_err());

    let denied_vendor = keys
        .sign_dispute_decision(case_id, Decision::Denied(Party::Vendor), None)
        .unwrap();
    let denied_buyer = keys
        .sign_dispute_decision(case_id, Decision::Denied(Party::Buyer), None)
        .unwrap();
    assert!(is_denial_reversed(&denied_vendor, &ruling, &signing_key));
    assert!(!is_denial_reversed(&denied_buyer, &ruling, &signing_key));
    assert!(!is_denial_reversed(&ruled, &ruling, &signing_key));
    let recused = keys.sign_dispute_decision(case_id, Decision::Recused, None).unwrap();
    assert_eq!(read_dispute_decision(&recused, &signing_key).unwrap().0, Decision::Recused);
    assert!(keys
        .sign_dispute_decision(case_id, Decision::Recused, Some(&ruling))
        .is_err());
    assert!(!is_denial_reversed(&recused, &ruling, &signing_key));
    let other_key = ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED)
        .unwrap()
        .signing_public();
    assert!(!is_denial_reversed(&denied_vendor, &ruling, &other_key));
}

#[test]
fn a_case_accepted_record_reads_only_under_its_signer() {
    let keys = moderator_keys();
    let bytes = keys.sign_case_accepted([4; 32], TIP + 1, CLAIM).unwrap();
    let accepted = read_case_accepted(&bytes, &keys.signing_public()).unwrap();
    assert_eq!((accepted.case_id, accepted.due_height), ([4; 32], CLAIM));
    let other = ModeratorKeys::from_seed(&OTHER_MODERATOR_SEED)
        .unwrap()
        .signing_public();
    assert_eq!(
        read_case_accepted(&bytes, &other).unwrap_err(),
        Error::ModeratorMismatch
    );
}
