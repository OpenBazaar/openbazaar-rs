//! MuSig2 key aggregation and the split adaptor secret.
//!
//! Curve operations go through `musig2` / `secp`. This module only hashes,
//! compares, and XORs.

use musig2::secp::{Point, Scalar};
use musig2::{
    AdaptorSignature, AggNonce, KeyAggContext, LiftedSignature, PartialSignature, PubNonce,
    SecNonce,
};
use rand::Rng;
use secp256k1::{Secp256k1, XOnlyPublicKey};

use crate::Error;

pub const MEMO_MAX: usize = 4096;
pub const SCRIPT_MAX: usize = 10_000;

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

pub fn hash32(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    use bitcoin::hashes::{sha256, Hash};
    let mut data = Vec::with_capacity(tag.len() + parts.iter().map(|p| p.len()).sum::<usize>());
    data.extend_from_slice(tag);
    for part in parts {
        data.extend_from_slice(part);
    }
    sha256::Hash::hash(&data).to_byte_array()
}

/// Derive the favored party's half of an adaptor secret from their long-term key.
///
/// The other party never learns this scalar. It is recomputed at settlement.
pub fn party_adaptor_secret(
    secret: &Scalar,
    label: &[u8],
    sighash: &[u8; 32],
) -> Result<Scalar, Error> {
    let key = secret.serialize();
    for counter in 0u32..8 {
        let n = counter.to_le_bytes();
        let digest = hash32(b"ob-escrow/party-adaptor/v1", &[&key, label, sighash, &n]);
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
    keys.sort_by(|left, right| left.serialize().cmp(&right.serialize()));
    KeyAggContext::new(keys).map_err(|_| Error::InvalidParams("key aggregation"))
}

pub fn output_xonly(ctx: &KeyAggContext) -> [u8; 32] {
    ctx.aggregated_pubkey::<Point>().serialize_xonly()
}

pub fn aggregate_nonce(left: &PubNonce, right: &PubNonce) -> AggNonce {
    [left, right].into_iter().sum()
}

pub fn fresh_nonce(seckey: Scalar, agg: &KeyAggContext, message: &[u8], extra: &[u8]) -> SecNonce {
    let pubkey: Point = agg.aggregated_pubkey();
    let mut rng = rand::rng();
    SecNonce::generate(&mut rng, seckey, pubkey, message, extra)
}

pub fn public_nonce(secnonce: &SecNonce) -> PubNonce {
    secnonce.public_nonce()
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

pub fn verify_plain_partial(
    ctx: &KeyAggContext,
    partial: PartialSignature,
    nonce: &AggNonce,
    pubkey: Point,
    pubnonce: &PubNonce,
    message: &[u8],
) -> Result<(), Error> {
    musig2::verify_partial(ctx, partial, nonce, pubkey, pubnonce, message)
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
    let pubkey: Point = ctx.aggregated_pubkey();
    musig2::adaptor::verify_single(pubkey, &signature, message, adaptor)
        .map_err(|_| Error::BadSignature)?;
    Ok(signature)
}

pub fn signature_bytes(signature: &LiftedSignature) -> [u8; 64] {
    signature.serialize()
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

/// Encrypt `secret` to `moderator` with an ephemeral ECDH key.
///
/// The MAC covers the settlement context so the moderator can see which
/// outpoint and payout the secret authorizes.
#[derive(Clone, Copy, Debug)]
pub struct EncryptedSecret {
    pub ephemeral_pk: [u8; 33],
    pub ciphertext: [u8; 32],
    pub mac: [u8; 32],
}

pub struct MacContext<'a> {
    pub adaptor_point: &'a [u8; 33],
    pub sighash: &'a [u8; 32],
    pub txid: &'a [u8; 32],
    pub vout: u32,
    pub lock_time: u32,
    pub value: u64,
    pub favored: u8,
    pub script: &'a [u8],
}

impl EncryptedSecret {
    pub fn seal(
        moderator: Point,
        secret: Scalar,
        ephemeral: Scalar,
        context: &MacContext<'_>,
    ) -> Self {
        let ephemeral_pk = ephemeral.base_point_mul().serialize();
        let shared = moderator * ephemeral;
        let pad = hash32(b"ob-escrow/ecdh/v1", &[&shared.serialize(), &ephemeral_pk]);
        let secret_bytes = secret.serialize();
        let mut ciphertext = [0u8; 32];
        for i in 0..32 {
            ciphertext[i] = secret_bytes[i] ^ pad[i];
        }
        let mac = mac(&pad, &ciphertext, context);
        Self {
            ephemeral_pk,
            ciphertext,
            mac,
        }
    }

    pub fn open(&self, moderator: Scalar, context: &MacContext<'_>) -> Result<Scalar, Error> {
        let ephemeral = Point::from_slice(&self.ephemeral_pk).map_err(|_| Error::BadPayload)?;
        let shared = ephemeral * moderator;
        let pad = hash32(
            b"ob-escrow/ecdh/v1",
            &[&shared.serialize(), &self.ephemeral_pk],
        );
        let mac = mac(&pad, &self.ciphertext, context);
        if !ct_eq(&mac, &self.mac) {
            return Err(Error::BadPayload);
        }
        let mut plain = [0u8; 32];
        for i in 0..32 {
            plain[i] = self.ciphertext[i] ^ pad[i];
        }
        Scalar::from_slice(&plain).map_err(|_| Error::BadPayload)
    }
}

fn mac(pad: &[u8; 32], ciphertext: &[u8; 32], context: &MacContext<'_>) -> [u8; 32] {
    let vout = context.vout.to_le_bytes();
    let lock_time = context.lock_time.to_le_bytes();
    let value = context.value.to_le_bytes();
    hash32(
        b"ob-escrow/mac/v1",
        &[
            pad,
            ciphertext,
            context.adaptor_point,
            context.sighash,
            context.txid,
            &vout,
            &lock_time,
            &value,
            &[context.favored],
            context.script,
        ],
    )
}

fn ct_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn cosign_plain(
    ctx: &KeyAggContext,
    buyer: Scalar,
    vendor: Scalar,
    message: &[u8],
    label: &str,
) -> Result<LiftedSignature, Error> {
    let (nonce, partials) = exchange(ctx, buyer, vendor, message, label, None)?;
    aggregate_plain(ctx, &nonce, partials, message)
}

pub fn cosign_adaptor(
    ctx: &KeyAggContext,
    buyer: Scalar,
    vendor: Scalar,
    message: &[u8],
    label: &str,
    adaptor: Point,
) -> Result<AdaptorSignature, Error> {
    let (nonce, partials) = exchange(ctx, buyer, vendor, message, label, Some(adaptor))?;
    aggregate_adaptor(ctx, &nonce, adaptor, partials, message)
}

fn exchange(
    ctx: &KeyAggContext,
    buyer: Scalar,
    vendor: Scalar,
    message: &[u8],
    label: &str,
    adaptor: Option<Point>,
) -> Result<(AggNonce, [PartialSignature; 2]), Error> {
    let buyer_extra = format!("ob-escrow/{label}/buyer");
    let vendor_extra = format!("ob-escrow/{label}/vendor");
    let buyer_sec = fresh_nonce(buyer, ctx, message, buyer_extra.as_bytes());
    let vendor_sec = fresh_nonce(vendor, ctx, message, vendor_extra.as_bytes());
    let buyer_pub = public_nonce(&buyer_sec);
    let vendor_pub = public_nonce(&vendor_sec);
    let nonce = aggregate_nonce(&buyer_pub, &vendor_pub);
    let buyer_partial = partial_sign(ctx, buyer, buyer_sec, &nonce, message, adaptor)?;
    let vendor_partial = partial_sign(ctx, vendor, vendor_sec, &nonce, message, adaptor)?;
    let buyer_point = buyer.base_point_mul();
    let vendor_point = vendor.base_point_mul();
    verify_branch_partial(
        ctx,
        buyer_partial,
        &nonce,
        buyer_point,
        &buyer_pub,
        message,
        adaptor,
    )?;
    verify_branch_partial(
        ctx,
        vendor_partial,
        &nonce,
        vendor_point,
        &vendor_pub,
        message,
        adaptor,
    )?;
    Ok((nonce, [buyer_partial, vendor_partial]))
}

fn verify_branch_partial(
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
                .map_err(|_| Error::BadPartial)
        }
        None => verify_plain_partial(ctx, partial, nonce, pubkey, pubnonce, message),
    }
}

pub fn encode_partial(partial: &PartialSignature) -> [u8; 32] {
    partial.serialize()
}

pub fn decode_partial(bytes: &[u8]) -> Result<PartialSignature, Error> {
    PartialSignature::from_slice(bytes).map_err(|_| Error::BadPartial)
}

pub fn point_from_compressed(bytes: &[u8; 33]) -> Result<Point, Error> {
    Point::from_slice(bytes).map_err(|_| Error::InvalidParams("public key"))
}

/// Add two adaptor halves. The sum is the discrete log of `T_party + T_mod`.
pub fn combine_adaptor_halves(party: Scalar, moderator: Scalar) -> Result<Scalar, Error> {
    (party + moderator)
        .not_zero()
        .map_err(|_| Error::BadSignature)
}
