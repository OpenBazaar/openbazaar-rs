//! MuSig2 key aggregation, partial signatures, and BIP 340 helpers.
//!
//! Curve operations go through `musig2` / `secp` / `libsecp256k1`. Every
//! signing helper takes exactly one secret key.

use musig2::secp::{Point, Scalar};
use musig2::{
    AdaptorSignature, AggNonce, KeyAggContext, LiftedSignature, PartialSignature, PubNonce,
    SecNonce,
};
use rand::Rng;
use secp256k1::{Keypair, Secp256k1, SecretKey, XOnlyPublicKey};

use crate::proofs::tagged;
use crate::Error;

pub fn random_scalar() -> Scalar {
    let mut rng = rand::rng();
    loop {
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes);
        if let Ok(scalar) = Scalar::from_slice(&bytes) {
            return scalar;
        }
    }
}

pub fn scalar_key(bytes: &[u8; 32]) -> Result<Scalar, Error> {
    Scalar::from_slice(bytes).map_err(|_| Error::WrongKey)
}

pub fn point_from_compressed(bytes: &[u8; 33]) -> Result<Point, Error> {
    Point::from_slice(bytes).map_err(|_| Error::InvalidParams("public key"))
}

/// The favored party's half of a branch adaptor secret.
///
/// Derived from that party's per-order key and the branch sighash, so it can
/// be recomputed at settlement and is never sent.
pub fn party_adaptor_secret(
    secret: &Scalar,
    label: &[u8],
    sighash: &[u8; 32],
) -> Result<Scalar, Error> {
    let key = secret.serialize();
    for counter in 0u32..8 {
        let digest = tagged(
            "ob-escrow/party-adaptor/v2",
            &[&key, label, sighash, &counter.to_le_bytes()],
        );
        if let Ok(scalar) = Scalar::from_slice(&digest) {
            return Ok(scalar);
        }
    }
    Err(Error::InvalidParams("adaptor scalar"))
}

pub fn aggregate(buyer: Point, vendor: Point) -> Result<KeyAggContext, Error> {
    sorted_context(buyer, vendor)?
        .with_unspendable_taproot_tweak()
        .map_err(|_| Error::KeyMismatch)
}

pub fn internal_xonly(buyer: Point, vendor: Point) -> Result<[u8; 32], Error> {
    Ok(sorted_context(buyer, vendor)?
        .aggregated_pubkey::<Point>()
        .serialize_xonly())
}

fn sorted_context(buyer: Point, vendor: Point) -> Result<KeyAggContext, Error> {
    if buyer == vendor {
        return Err(Error::InvalidParams("buyer and vendor keys must differ"));
    }
    let mut keys = [buyer, vendor];
    keys.sort_by_key(|key| key.serialize());
    KeyAggContext::new(keys).map_err(|_| Error::InvalidParams("key aggregation"))
}

pub fn output_xonly(ctx: &KeyAggContext) -> [u8; 32] {
    ctx.aggregated_pubkey::<Point>().serialize_xonly()
}

pub fn aggregate_nonce(buyer: &PubNonce, vendor: &PubNonce) -> AggNonce {
    [buyer, vendor].into_iter().sum()
}

pub fn fresh_nonce(seckey: Scalar, agg: &KeyAggContext, message: &[u8], extra: &[u8]) -> SecNonce {
    let pubkey: Point = agg.aggregated_pubkey();
    let mut rng = rand::rng();
    SecNonce::generate(&mut rng, seckey, pubkey, message, extra)
}

pub fn partial_sign(
    ctx: &KeyAggContext,
    seckey: Scalar,
    secnonce: SecNonce,
    nonce: &AggNonce,
    message: &[u8],
    adaptor: Option<Point>,
) -> Result<PartialSignature, Error> {
    match adaptor {
        Some(point) => musig2::adaptor::sign_partial(ctx, seckey, secnonce, nonce, point, message)
            .map_err(|_| Error::BadPartial),
        None => musig2::sign_partial(ctx, seckey, secnonce, nonce, message)
            .map_err(|_| Error::BadPartial),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn verify_partial(
    ctx: &KeyAggContext,
    partial: PartialSignature,
    nonce: &AggNonce,
    pubkey: Point,
    pubnonce: &PubNonce,
    message: &[u8],
    adaptor: Option<Point>,
) -> Result<(), Error> {
    match adaptor {
        Some(point) => {
            musig2::adaptor::verify_partial(ctx, partial, nonce, point, pubkey, pubnonce, message)
        }
        None => musig2::verify_partial(ctx, partial, nonce, pubkey, pubnonce, message),
    }
    .map_err(|_| Error::BadPartial)
}

pub fn aggregate_plain(
    ctx: &KeyAggContext,
    nonce: &AggNonce,
    partials: [PartialSignature; 2],
    message: &[u8],
) -> Result<LiftedSignature, Error> {
    musig2::aggregate_partial_signatures(ctx, nonce, partials, message)
        .map_err(|_| Error::BadSignature)
}

pub fn aggregate_adaptor(
    ctx: &KeyAggContext,
    nonce: &AggNonce,
    adaptor: Point,
    partials: [PartialSignature; 2],
    message: &[u8],
) -> Result<AdaptorSignature, Error> {
    let signature =
        musig2::adaptor::aggregate_partial_signatures(ctx, nonce, adaptor, partials, message)
            .map_err(|_| Error::BadSignature)?;
    verify_adaptor(ctx, &signature, message, adaptor)?;
    Ok(signature)
}

pub fn verify_adaptor(
    ctx: &KeyAggContext,
    signature: &AdaptorSignature,
    message: &[u8],
    adaptor: Point,
) -> Result<(), Error> {
    let pubkey: Point = ctx.aggregated_pubkey();
    musig2::adaptor::verify_single(pubkey, signature, message, adaptor)
        .map_err(|_| Error::BadSignature)
}

pub fn bip340_verify(
    output_xonly: &[u8; 32],
    signature: &[u8; 64],
    message: &[u8; 32],
) -> Result<(), Error> {
    let secp = Secp256k1::verification_only();
    let key = XOnlyPublicKey::from_byte_array(*output_xonly).map_err(|_| Error::BadSignature)?;
    let sig = secp256k1::schnorr::Signature::from_byte_array(*signature);
    secp.verify_schnorr(&sig, message, &key)
        .map_err(|_| Error::BadSignature)
}

/// BIP 340 signature by a single key over a 32-byte digest.
pub fn schnorr_sign(secret: &Scalar, digest: &[u8; 32]) -> Result<[u8; 64], Error> {
    let secp = Secp256k1::signing_only();
    let key = SecretKey::from_byte_array(secret.serialize()).map_err(|_| Error::WrongKey)?;
    let pair = Keypair::from_secret_key(&secp, &key);
    let mut aux = [0u8; 32];
    rand::rng().fill(&mut aux);
    Ok(secp
        .sign_schnorr_with_aux_rand(digest, &pair, &aux)
        .to_byte_array())
}

/// Verify a [`schnorr_sign`] signature against a compressed key's x-coordinate.
pub fn schnorr_verify(key: &Point, digest: &[u8; 32], signature: &[u8; 64]) -> Result<(), Error> {
    bip340_verify(&key.serialize_xonly(), signature, digest)
}
