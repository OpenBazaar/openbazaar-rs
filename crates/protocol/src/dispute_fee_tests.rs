use bitcoin::secp256k1::SecretKey;

use crate::sign::{secret_from_label, xonly};
use crate::{
    CaseAccepted, DisputeDecision, Error, FeeInvoice, FeePayment, FeeReceipt, FeeRequest, Signed, SignedCaseAccepted,
    SignedDisputeDecision, SignedDisputeOutcome, SignedFeeReceipt, SignedFeeRequest, Terms, DECISION_BUYER,
    DECISION_DENIED_BUYER, DECISION_DENIED_VENDOR, DECISION_VENDOR, FAVORED_BUYER, FAVORED_VENDOR,
};

fn moderator() -> SecretKey {
    secret_from_label("dispute-fee-moderator")
}

fn request() -> FeeRequest {
    FeeRequest {
        moderator_key: xonly(&moderator()),
        case_id: [1; 32],
        letter_digest: [2; 32],
        favored: FAVORED_VENDOR,
        amount_sats: 5_000,
        invoice: Some(FeeInvoice {
            network: "signet".into(),
            bolt11: "lntbs50u1pexample".into(),
            payment_hash: [3; 32],
        }),
        script: vec![
            0x51, 0x20, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
        ],
        valid_until_height: 900_100,
        due_height: 902_160,
    }
}

fn receipt() -> FeeReceipt {
    FeeReceipt {
        moderator_key: xonly(&moderator()),
        case_id: [1; 32],
        request_digest: [5; 32],
        payment: FeePayment::OnChain { txid: [6; 32], vout: 1 },
        received_sats: 5_000,
        paid_height: 900_010,
        due_height: 902_160,
    }
}

fn accepted() -> CaseAccepted {
    CaseAccepted {
        moderator_key: xonly(&moderator()),
        case_id: [1; 32],
        accepted_height: 900_010,
        due_height: 902_160,
    }
}

fn decision(decision: u8, ruling_digest: [u8; 32]) -> DisputeDecision {
    DisputeDecision {
        moderator_key: xonly(&moderator()),
        case_id: [1; 32],
        decision,
        ruling_digest,
    }
}

/// Round trip, and flipping any byte of the body or signature fails decode or verify.
fn round_trip<T: Terms + Clone + PartialEq + core::fmt::Debug>(terms: T, magic: &[u8; 4]) {
    let signed = Signed::sign(terms.clone(), &moderator()).unwrap();
    let bytes = signed.encode().unwrap();
    assert_eq!(&bytes[..5], &[magic[0], magic[1], magic[2], magic[3], 1]);
    let again = Signed::<T>::decode(&bytes).unwrap();
    again.verify().unwrap();
    assert_eq!(again.terms, terms);
    for index in 0..bytes.len() {
        let mut tampered = bytes.clone();
        tampered[index] ^= 0x01;
        let accepted = Signed::<T>::decode(&tampered).and_then(|signed| signed.verify().map(|_| signed));
        assert!(accepted.is_err(), "{magic:?}: byte {index} is covered");
    }
}

#[test]
fn every_record_round_trips_and_every_byte_is_signed() {
    round_trip(request(), b"OBFQ");
    round_trip(
        FeeRequest {
            invoice: None,
            ..request()
        },
        b"OBFQ",
    );
    round_trip(receipt(), b"OBFR");
    round_trip(
        FeeReceipt {
            payment: FeePayment::Lightning { payment_hash: [7; 32] },
            ..receipt()
        },
        b"OBFR",
    );
    round_trip(accepted(), b"OBCA");
    round_trip(decision(DECISION_VENDOR, [8; 32]), b"OBDD");
    round_trip(decision(DECISION_DENIED_BUYER, [0; 32]), b"OBDD");
}

#[test]
fn only_the_named_moderator_signs() {
    let other = secret_from_label("someone-else");
    assert_eq!(Signed::sign(request(), &other).unwrap_err(), Error::BadKey);
    assert_eq!(Signed::sign(accepted(), &other).unwrap_err(), Error::BadKey);
    let mut forged = Signed::sign(accepted(), &moderator()).unwrap();
    forged.terms.moderator_key = xonly(&other);
    assert_eq!(forged.verify().unwrap_err(), Error::BadSignature);
}

#[test]
fn a_request_needs_a_filer_an_amount_a_script_and_ordered_heights() {
    let refused = [
        FeeRequest {
            favored: 0,
            ..request()
        },
        FeeRequest {
            amount_sats: 0,
            ..request()
        },
        FeeRequest {
            script: Vec::new(),
            ..request()
        },
        FeeRequest {
            script: vec![0; 65],
            ..request()
        },
        FeeRequest {
            valid_until_height: 902_161,
            ..request()
        },
        FeeRequest {
            valid_until_height: 0,
            ..request()
        },
        FeeRequest {
            invoice: Some(FeeInvoice {
                network: String::new(),
                bolt11: "lnbc1".into(),
                payment_hash: [0; 32],
            }),
            ..request()
        },
        FeeRequest {
            invoice: Some(FeeInvoice {
                network: "signet".into(),
                bolt11: "ln bc".into(),
                payment_hash: [0; 32],
            }),
            ..request()
        },
    ];
    for terms in refused {
        assert!(Signed::sign(terms.clone(), &moderator()).is_err(), "{terms:?}");
    }
    Signed::sign(
        FeeRequest {
            favored: FAVORED_BUYER,
            ..request()
        },
        &moderator(),
    )
    .unwrap();
}

#[test]
fn a_decision_cites_a_ruling_exactly_when_it_rules() {
    for terms in [
        decision(DECISION_BUYER, [0; 32]),
        decision(DECISION_DENIED_VENDOR, [8; 32]),
        decision(0, [8; 32]),
        decision(5, [0; 32]),
    ] {
        assert!(Signed::sign(terms.clone(), &moderator()).is_err(), "{terms:?}");
    }
    let denied = decision(DECISION_DENIED_VENDOR, [0; 32]);
    assert!(denied.is_denial());
    assert_eq!(denied.filer(), FAVORED_VENDOR);
    assert_eq!(decision(DECISION_BUYER, [8; 32]).filer(), FAVORED_BUYER);
}

#[test]
fn public_records_refuse_text_in_their_ids() {
    let mut street = [0u8; 32];
    street[..15].copy_from_slice(b"12 Cedar Street");
    assert!(Signed::sign(
        CaseAccepted {
            case_id: street,
            ..accepted()
        },
        &moderator()
    )
    .is_err());
    assert!(Signed::sign(
        DisputeDecision {
            case_id: street,
            ..decision(DECISION_BUYER, [8; 32])
        },
        &moderator()
    )
    .is_err());
}

#[test]
fn records_do_not_decode_as_one_another() {
    let accepted = SignedCaseAccepted::sign(accepted(), &moderator())
        .unwrap()
        .encode()
        .unwrap();
    let decision = SignedDisputeDecision::sign(decision(DECISION_BUYER, [8; 32]), &moderator())
        .unwrap()
        .encode()
        .unwrap();
    assert_eq!(SignedDisputeDecision::decode(&accepted).unwrap_err(), Error::BadMagic);
    assert_eq!(SignedCaseAccepted::decode(&decision).unwrap_err(), Error::BadMagic);
    assert_eq!(SignedDisputeOutcome::decode(&decision).unwrap_err(), Error::BadMagic);
    assert_eq!(SignedFeeReceipt::decode(&accepted).unwrap_err(), Error::BadMagic);
    assert!(SignedFeeRequest::decode(&decision).is_err());
}
