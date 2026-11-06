use crate::{
    fund, is_final_at_height, moderator_open, Claim, Error, Escrow, FundParams, Party, Phase,
};
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Txid};
use secp256k1::{Keypair, Secp256k1, SecretKey};

fn secret(byte: u8) -> [u8; 32] {
    [byte; 32]
}

fn payout(secret_bytes: &[u8; 32]) -> ScriptBuf {
    let secp = Secp256k1::signing_only();
    let key = SecretKey::from_byte_array(*secret_bytes).unwrap();
    let (xonly, _) = key.x_only_public_key(&secp);
    let xonly = bitcoin::secp256k1::XOnlyPublicKey::from_slice(&xonly.serialize()).unwrap();
    let bitcoin_secp = bitcoin::secp256k1::Secp256k1::verification_only();
    ScriptBuf::new_p2tr(&bitcoin_secp, xonly, None)
}

fn params() -> (FundParams, [u8; 32], [u8; 32], [u8; 32]) {
    let buyer = secret(0x11);
    let vendor = secret(0x22);
    let moderator = secret(0x33);
    let moderator_public = {
        let secp = Secp256k1::signing_only();
        let key = SecretKey::from_byte_array(moderator).unwrap();
        key.public_key(&secp).serialize()
    };
    let params = FundParams {
        buyer_secret: buyer,
        vendor_secret: vendor,
        moderator_public,
        funding_outpoint: OutPoint {
            txid: Txid::from_byte_array([0xab; 32]),
            vout: 1,
        },
        funding_value: Amount::from_sat(50_000),
        fee: Amount::from_sat(300),
        timeout_height: 100,
        buyer_payout: payout(&buyer),
        vendor_payout: payout(&vendor),
    };
    (params, buyer, vendor, moderator)
}

fn funded() -> (Escrow, [u8; 32], [u8; 32], [u8; 32], ScriptBuf, ScriptBuf) {
    let (p, buyer, vendor, moderator) = params();
    let buyer_payout = p.buyer_payout.clone();
    let vendor_payout = p.vendor_payout.clone();
    let escrow = fund(p).unwrap();
    (
        escrow,
        buyer,
        vendor,
        moderator,
        buyer_payout,
        vendor_payout,
    )
}

fn release(escrow: &mut Escrow, buyer: &[u8; 32], vendor: &[u8; 32]) -> bitcoin::Transaction {
    let buyer_nonce = escrow.release_nonce(buyer).unwrap();
    let vendor_nonce = escrow.release_nonce(vendor).unwrap();
    let buyer_pub = buyer_nonce.nonce.clone();
    let vendor_pub = vendor_nonce.nonce.clone();
    let buyer_partial = escrow
        .release_partial(buyer, buyer_nonce, &vendor_pub)
        .unwrap();
    let vendor_partial = escrow
        .release_partial(vendor, vendor_nonce, &buyer_pub)
        .unwrap();
    escrow
        .complete_release(buyer_partial, vendor_partial)
        .unwrap()
}

#[test]
fn happy_path_aggregate_verifies_against_the_funding_output() {
    let (mut escrow, buyer, vendor, _, _, vendor_payout) = funded();
    let output_key = escrow.output_key();
    assert_eq!(tx_key(&escrow), Some(output_key));
    let tx = release(&mut escrow, &buyer, &vendor);
    assert_eq!(escrow.phase(), Phase::Completed);
    escrow.verify_key_spend(&tx).unwrap();
    assert_eq!(tx.output[0].script_pubkey, vendor_payout);
    assert_eq!(tx.output[0].value, Amount::from_sat(49_700));
    let signature = tx.input[0].witness.iter().next().unwrap();
    assert_eq!(signature.len(), 64);
    assert_eq!(tx.input[0].witness.len(), 1);
    assert!(is_final_at_height(&tx, 1));
    assert!(!bitcoin::consensus::serialize(&tx).is_empty());
    assert_eq!(tx.lock_time, LockTime::ZERO);
}

#[test]
fn one_party_cannot_sign_the_happy_path() {
    let (escrow, buyer, vendor, moderator, _, _) = funded();
    let buyer_nonce = escrow.release_nonce(&buyer).unwrap();
    let vendor_nonce = escrow.release_nonce(&vendor).unwrap();
    let buyer_pub = buyer_nonce.nonce.clone();
    let vendor_pub = vendor_nonce.nonce.clone();
    let buyer_partial = escrow
        .release_partial(&buyer, buyer_nonce, &vendor_pub)
        .unwrap();
    let vendor_partial = escrow
        .release_partial(&vendor, vendor_nonce, &buyer_pub)
        .unwrap();

    let mut cloned = escrow.clone();
    let err = cloned
        .complete_release(buyer_partial.clone(), buyer_partial.clone())
        .unwrap_err();
    assert_eq!(err, Error::WrongKey);

    let mut cloned = escrow.clone();
    let mut forged = vendor_partial.clone();
    forged
        .set_partial_bytes(buyer_partial.partial_bytes())
        .unwrap();
    let err = cloned.complete_release(buyer_partial, forged).unwrap_err();
    assert_eq!(err, Error::BadPartial);

    assert!(matches!(
        escrow.release_nonce(&moderator),
        Err(Error::WrongKey)
    ));

    let secp = Secp256k1::new();
    let key = SecretKey::from_byte_array(buyer).unwrap();
    let pair = Keypair::from_secret_key(&secp, &key);
    let message = escrow.release_message();
    let solo = secp.sign_schnorr(&message, &pair);
    let output = secp256k1::XOnlyPublicKey::from_byte_array(escrow.output_key()).unwrap();
    assert!(secp.verify_schnorr(&solo, &message, &output).is_err());
}

#[test]
fn tampered_partial_signature_fails() {
    let (mut escrow, buyer, vendor, _, _, _) = funded();
    let buyer_nonce = escrow.release_nonce(&buyer).unwrap();
    let vendor_nonce = escrow.release_nonce(&vendor).unwrap();
    let buyer_pub = buyer_nonce.nonce.clone();
    let vendor_pub = vendor_nonce.nonce.clone();
    let buyer_partial = escrow
        .release_partial(&buyer, buyer_nonce, &vendor_pub)
        .unwrap();
    let mut vendor_partial = escrow
        .release_partial(&vendor, vendor_nonce, &buyer_pub)
        .unwrap();

    let original = vendor_partial.partial_bytes();
    let mut mutated = original;
    let mut found = false;
    for i in 0..32 {
        mutated[i] ^= 0x01;
        if vendor_partial.set_partial_bytes(mutated).is_ok() && mutated != original {
            found = true;
            break;
        }
        mutated[i] ^= 0x01;
    }
    assert!(found, "a one-bit change should still be a scalar sometimes");
    let err = escrow
        .complete_release(buyer_partial, vendor_partial)
        .unwrap_err();
    assert_eq!(err, Error::BadPartial);
    assert_eq!(escrow.phase(), Phase::Funded);
}

#[test]
fn refund_is_locked_until_the_timeout_and_pays_the_buyer() {
    let (mut escrow, _, _, _, buyer_payout, _) = funded();
    let refund = escrow.refund_transaction().clone();
    assert_eq!(refund.output[0].script_pubkey, buyer_payout);
    assert_eq!(refund.output[0].value, Amount::from_sat(49_700));
    assert_eq!(refund.input[0].sequence, crate::tx::LOCKTIME_SEQUENCE);
    assert_ne!(refund.input[0].sequence, bitcoin::Sequence::MAX);
    escrow.verify_key_spend(&refund).unwrap();
    assert_eq!(refund.input[0].witness.iter().next().unwrap().len(), 64);

    assert!(!is_final_at_height(&refund, 100));
    assert!(!is_final_at_height(&refund, 99));
    assert!(is_final_at_height(&refund, 101));
    assert_eq!(
        escrow.refund(100).unwrap_err(),
        Error::LockTimeNotSatisfied {
            lock_time: 100,
            height: 100
        }
    );
    assert_eq!(escrow.phase(), Phase::Funded);

    let broadcast = escrow.refund(101).unwrap();
    assert_eq!(escrow.phase(), Phase::Refunded);
    escrow.verify_key_spend(&broadcast).unwrap();
    assert_eq!(broadcast.output[0].script_pubkey, buyer_payout);
}

#[test]
fn moderator_alone_cannot_settle_and_either_pair_can() {
    let (escrow, buyer, vendor, moderator, buyer_payout, vendor_payout) = funded();
    assert_eq!(tx_key(&escrow), Some(escrow.output_key()));

    let buyer_letter = escrow
        .dispute_payload(&Claim {
            favored: Party::Buyer,
            memo: b"vendor never shipped".to_vec(),
        })
        .unwrap();
    let vendor_letter = escrow
        .dispute_payload(&Claim {
            favored: Party::Vendor,
            memo: b"buyer disappeared".to_vec(),
        })
        .unwrap();
    assert!(buyer_letter.windows(13).any(|w| w == b"never shipped"));
    assert!(buyer_letter
        .windows(32)
        .any(|w| w == escrow.funding_outpoint().txid.as_byte_array()));

    assert_eq!(
        moderator_open(&buyer_letter, &buyer).unwrap_err(),
        Error::BadPayload
    );
    assert_eq!(
        moderator_open(&buyer_letter, &vendor).unwrap_err(),
        Error::BadPayload
    );

    let buyer_share = moderator_open(&buyer_letter, &moderator).unwrap();
    let vendor_share = moderator_open(&vendor_letter, &moderator).unwrap();

    // The moderator half is real adaptor material, and it is not sufficient.
    assert!(escrow
        .adapt_for_test(Party::Buyer, buyer_share.scalar())
        .is_err());
    assert!(escrow
        .adapt_for_test(Party::Vendor, vendor_share.scalar())
        .is_err());

    let mut buyer_side = escrow.clone();
    let err = buyer_side
        .settle_dispute(&buyer_letter, &buyer_share, &moderator)
        .unwrap_err();
    assert_eq!(err, Error::WrongKey);
    assert_eq!(buyer_side.phase(), Phase::Funded);

    let err = buyer_side
        .settle_dispute(&buyer_letter, &buyer_share, &vendor)
        .unwrap_err();
    assert!(matches!(err, Error::WrongKey | Error::BadSignature));

    let buyer_tx = buyer_side
        .settle_dispute(&buyer_letter, &buyer_share, &buyer)
        .unwrap();
    assert_eq!(buyer_side.phase(), Phase::Disputed);
    buyer_side.verify_key_spend(&buyer_tx).unwrap();
    assert_eq!(buyer_tx.output[0].script_pubkey, buyer_payout);
    assert_eq!(buyer_tx.input[0].witness.len(), 1);
    assert_eq!(buyer_tx.input[0].witness.iter().next().unwrap().len(), 64);
    assert!(is_final_at_height(&buyer_tx, 1));

    let mut vendor_side = escrow.clone();
    let vendor_tx = vendor_side
        .settle_dispute(&vendor_letter, &vendor_share, &vendor)
        .unwrap();
    vendor_side.verify_key_spend(&vendor_tx).unwrap();
    assert_eq!(vendor_tx.output[0].script_pubkey, vendor_payout);
    assert_eq!(vendor_tx.input[0].witness.iter().next().unwrap().len(), 64);
    assert_eq!(buyer_side.output_key(), vendor_side.output_key());

    let mut flipped = buyer_letter.clone();
    flipped[80] ^= 0x01;
    assert!(moderator_open(&flipped, &moderator).is_err());
}

#[test]
fn invalid_transitions_leave_the_phase_unchanged() {
    let (escrow, buyer, vendor, moderator, _, _) = funded();

    let mut completed = escrow.clone();
    release(&mut completed, &buyer, &vendor);
    assert_eq!(
        completed.refund(101).unwrap_err(),
        Error::InvalidTransition {
            from: Phase::Completed,
            to: Phase::Refunded
        }
    );
    let letter = completed
        .dispute_payload(&Claim {
            favored: Party::Buyer,
            memo: vec![],
        })
        .unwrap();
    let share = moderator_open(&letter, &moderator).unwrap();
    assert!(matches!(
        completed
            .settle_dispute(&letter, &share, &buyer)
            .unwrap_err(),
        Error::InvalidTransition { .. }
    ));
    assert_eq!(completed.phase(), Phase::Completed);

    let mut refunded = escrow.clone();
    refunded.refund(101).unwrap();
    let buyer_nonce = refunded.release_nonce(&buyer).unwrap();
    let vendor_nonce = refunded.release_nonce(&vendor).unwrap();
    let buyer_pub = buyer_nonce.nonce.clone();
    let vendor_pub = vendor_nonce.nonce.clone();
    let buyer_partial = refunded
        .release_partial(&buyer, buyer_nonce, &vendor_pub)
        .unwrap();
    let vendor_partial = refunded
        .release_partial(&vendor, vendor_nonce, &buyer_pub)
        .unwrap();
    assert!(matches!(
        refunded
            .complete_release(buyer_partial, vendor_partial)
            .unwrap_err(),
        Error::InvalidTransition {
            from: Phase::Refunded,
            to: Phase::Completed
        }
    ));

    let mut disputed = escrow;
    let letter = disputed
        .dispute_payload(&Claim {
            favored: Party::Vendor,
            memo: b"claim".to_vec(),
        })
        .unwrap();
    let share = moderator_open(&letter, &moderator).unwrap();
    disputed.settle_dispute(&letter, &share, &vendor).unwrap();
    assert!(matches!(
        disputed.refund(101).unwrap_err(),
        Error::InvalidTransition {
            from: Phase::Disputed,
            to: Phase::Refunded
        }
    ));
}

fn tx_key(escrow: &Escrow) -> Option<[u8; 32]> {
    crate::tx::p2tr_output_key(&escrow.funding_output().script_pubkey)
}
