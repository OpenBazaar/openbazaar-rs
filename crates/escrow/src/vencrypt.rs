//! Verifiable encryption of a moderator share to the moderator key.
//!
//! The share `t` (with public point `T = tG`) is split into 256 bits `b_i`.
//! Each bit is exponential-ElGamal encrypted to the moderator key `M`:
//! `(A_i, B_i) = (r_i G, r_i M + b_i G)`, with a [`BitProof`] that `b_i` is
//! 0 or 1. A [`Dleq`] then shows `log_G(Σ 2^i A_i) = log_M(Σ 2^i B_i - T)`,
//! so the bits sum to `t` mod `n`. Anyone holding `M` and `T` can check the
//! ciphertext. Only the holder of `m` can decrypt: `B_i - m A_i` is either
//! the point at infinity or `G`.
//!
//! The ciphertext is about 50 KB. That is the cost of proving the plaintext
//! without a general-purpose zero-knowledge system.

use musig2::secp::{MaybePoint, MaybeScalar, Point, Scalar, G};

use crate::crypto::random_scalar;
use crate::proofs::{tagged, BitProof, Dleq};
use crate::wire::{Reader, Writer};
use crate::Error;

const BITS: usize = 256;
const DIGEST_TAG: &str = "ob-escrow/vencrypt/v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiableShare {
    point: Point,
    ciphertexts: Vec<(Point, Point)>,
    bit_proofs: Vec<BitProof>,
    link: Dleq,
}

impl VerifiableShare {
    /// Encrypt `secret` to `moderator`. `ctx` binds the ciphertext to one branch.
    pub fn encrypt(secret: Scalar, moderator: &Point, ctx: &[u8; 32]) -> Result<Self, Error> {
        let point = secret * G;
        let bits = bits_le(&secret);
        let mut randomness = Vec::with_capacity(BITS);
        let mut ciphertexts = Vec::with_capacity(BITS);
        for bit in bits {
            let r = random_scalar();
            let a = r * G;
            let masked = r * *moderator;
            let b = if bit {
                (masked + G).not_inf().map_err(|_| Error::BadProof)?
            } else {
                masked
            };
            randomness.push(r);
            ciphertexts.push((a, b));
        }
        let digest = digest(ctx, moderator, &point, &ciphertexts);
        let bit_proofs = bits
            .iter()
            .zip(&randomness)
            .zip(&ciphertexts)
            .enumerate()
            .map(|(i, ((bit, r), (a, b)))| {
                BitProof::prove(*bit, *r, moderator, a, b, &digest, i as u32)
            })
            .collect();
        let rho = horner_scalar(&randomness)
            .not_zero()
            .map_err(|_| Error::BadProof)?;
        let (a_sum, target) = link_points(&ciphertexts, &point)?;
        let link = Dleq::prove(rho, moderator, &a_sum, &target, &digest);
        Ok(Self {
            point,
            ciphertexts,
            bit_proofs,
            link,
        })
    }

    /// The share's public point `T`.
    pub fn point(&self) -> Point {
        self.point
    }

    /// Check every bit proof and the link to `T`.
    pub fn verify(&self, moderator: &Point, ctx: &[u8; 32]) -> Result<(), Error> {
        if self.ciphertexts.len() != BITS || self.bit_proofs.len() != BITS {
            return Err(Error::BadProof);
        }
        let digest = digest(ctx, moderator, &self.point, &self.ciphertexts);
        for (i, ((a, b), proof)) in self.ciphertexts.iter().zip(&self.bit_proofs).enumerate() {
            proof.verify(moderator, a, b, &digest, i as u32)?;
        }
        let (a_sum, target) = link_points(&self.ciphertexts, &self.point)?;
        self.link.verify(moderator, &a_sum, &target, &digest)
    }

    /// Verify, then decrypt with the moderator secret.
    pub fn open(&self, moderator: Scalar, ctx: &[u8; 32]) -> Result<Scalar, Error> {
        self.verify(&(moderator * G), ctx)?;
        let one = MaybePoint::from(Point::generator());
        let mut acc = MaybeScalar::Zero;
        for (a, b) in self.ciphertexts.iter().rev() {
            let plain = *b - moderator * *a;
            acc = acc + acc;
            if plain == one {
                acc += Scalar::one();
            } else if !plain.is_infinity() {
                return Err(Error::BadProof);
            }
        }
        let secret = acc.not_zero().map_err(|_| Error::BadProof)?;
        if secret * G != self.point {
            return Err(Error::BadProof);
        }
        Ok(secret)
    }

    pub fn write(&self, w: &mut Writer) {
        w.point(&self.point);
        for ((a, b), proof) in self.ciphertexts.iter().zip(&self.bit_proofs) {
            w.point(a);
            w.point(b);
            proof.write(w);
        }
        self.link.write(w);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let point = r.point()?;
        let mut ciphertexts = Vec::with_capacity(BITS);
        let mut bit_proofs = Vec::with_capacity(BITS);
        for _ in 0..BITS {
            ciphertexts.push((r.point()?, r.point()?));
            bit_proofs.push(BitProof::read(r)?);
        }
        Ok(Self {
            point,
            ciphertexts,
            bit_proofs,
            link: Dleq::read(r)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn corrupt_bit(&mut self, index: usize) {
        let (a, b) = self.ciphertexts[index];
        self.ciphertexts[index] = (a, (b + G).not_inf().expect("not infinity"));
    }
}

fn bits_le(secret: &Scalar) -> [bool; BITS] {
    let bytes = secret.serialize();
    let mut bits = [false; BITS];
    for (i, bit) in bits.iter_mut().enumerate() {
        *bit = (bytes[31 - i / 8] >> (i % 8)) & 1 == 1;
    }
    bits
}

/// `Σ 2^i x_i`, most significant term first.
fn horner_scalar(values: &[Scalar]) -> MaybeScalar {
    values
        .iter()
        .rev()
        .fold(MaybeScalar::Zero, |acc, value| acc + acc + *value)
}

fn horner_point(points: impl DoubleEndedIterator<Item = Point>) -> MaybePoint {
    points
        .rev()
        .fold(MaybePoint::Infinity, |acc, point| acc + acc + point)
}

/// `(Σ 2^i A_i, Σ 2^i B_i - T)`.
fn link_points(ciphertexts: &[(Point, Point)], point: &Point) -> Result<(Point, Point), Error> {
    let a_sum = horner_point(ciphertexts.iter().map(|(a, _)| *a))
        .not_inf()
        .map_err(|_| Error::BadProof)?;
    let target = (horner_point(ciphertexts.iter().map(|(_, b)| *b)) - *point)
        .not_inf()
        .map_err(|_| Error::BadProof)?;
    Ok((a_sum, target))
}

fn digest(
    ctx: &[u8; 32],
    moderator: &Point,
    point: &Point,
    ciphertexts: &[(Point, Point)],
) -> [u8; 32] {
    let mut body = Vec::with_capacity(ciphertexts.len() * 66);
    for (a, b) in ciphertexts {
        body.extend_from_slice(&a.serialize());
        body.extend_from_slice(&b.serialize());
    }
    tagged(
        DIGEST_TAG,
        &[ctx, &moderator.serialize(), &point.serialize(), &body],
    )
}
