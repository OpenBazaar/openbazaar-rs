use musig2::secp::G;

use crate::crypto::random_scalar;
use crate::proofs::{Dleq, Pok};
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer};
use crate::Error;

const CTX: [u8; 32] = [7; 32];
const OTHER_CTX: [u8; 32] = [8; 32];

#[test]
fn verifiable_share_round_trips_and_only_the_moderator_opens_it() {
    let moderator = random_scalar();
    let secret = random_scalar();
    let share = VerifiableShare::encrypt(secret, &(moderator * G), &CTX).unwrap();
    assert_eq!(share.point(), secret * G);
    share.verify(&(moderator * G), &CTX).unwrap();

    let mut w = Writer::new(b"TEST", 1);
    share.write(&mut w);
    let bytes = w.finish();
    let mut r = Reader::new(&bytes, b"TEST", 1).unwrap();
    let decoded = VerifiableShare::read(&mut r).unwrap();
    r.finish().unwrap();
    assert_eq!(decoded, share);
    assert_eq!(decoded.open(moderator).unwrap(), secret);

    assert_eq!(
        share.verify(&(moderator * G), &OTHER_CTX),
        Err(Error::BadProof)
    );
    let stranger = random_scalar();
    assert_eq!(share.verify(&(stranger * G), &CTX), Err(Error::BadProof));
    assert!(share.open(stranger).is_err());
}

#[test]
fn a_share_for_a_different_point_or_a_flipped_bit_is_rejected() {
    let moderator = random_scalar() * G;
    let mut flipped = VerifiableShare::encrypt(random_scalar(), &moderator, &CTX).unwrap();
    flipped.corrupt_bit(0);
    assert_eq!(flipped.verify(&moderator, &CTX), Err(Error::BadProof));

    // Splice a valid ciphertext under a different claimed point.
    let a = VerifiableShare::encrypt(random_scalar(), &moderator, &CTX).unwrap();
    let b = VerifiableShare::encrypt(random_scalar(), &moderator, &CTX).unwrap();
    let mut wa = Writer::new(b"TEST", 1);
    a.write(&mut wa);
    let mut bytes = wa.finish();
    bytes[5..38].copy_from_slice(&b.point().serialize());
    let mut r = Reader::new(&bytes, b"TEST", 1).unwrap();
    let spliced = VerifiableShare::read(&mut r).unwrap();
    assert_eq!(spliced.verify(&moderator, &CTX), Err(Error::BadProof));
}

#[test]
fn proof_of_knowledge_and_dleq_bind_their_statements() {
    let x = random_scalar();
    let pok = Pok::prove(x, &CTX);
    pok.verify(&(x * G), &CTX).unwrap();
    assert!(pok.verify(&(x * G), &OTHER_CTX).is_err());
    assert!(pok.verify(&(random_scalar() * G), &CTX).is_err());

    let h = random_scalar() * G;
    let a = x * G;
    let b = x * h;
    let proof = Dleq::prove(x, &h, &a, &b, &CTX);
    proof.verify(&h, &a, &b, &CTX).unwrap();
    let wrong = random_scalar() * h;
    assert!(proof.verify(&h, &a, &wrong, &CTX).is_err());
}
