//! Non-interactive sigma proofs over secp256k1.
//!
//! * [`Pok`]: Schnorr proof of knowledge of `x` with `X = xG`.
//! * [`Dleq`]: Chaum-Pedersen proof that `log_G(A) = log_H(B)`.
//! * [`BitProof`]: Cramer-Damgård-Schoenmakers OR of two Chaum-Pedersen
//!   statements, proving an exponential ElGamal ciphertext encrypts 0 or 1.
//!
//! Every challenge is a BIP 340-style tagged SHA-256 over length-prefixed
//! inputs, reduced mod `n`. Each proof type has its own tag, and callers pass a
//! 32-byte context that binds the proof to one escrow branch.

use bitcoin::hashes::{sha256, Hash, HashEngine};
use musig2::secp::{MaybePoint, MaybeScalar, Point, Scalar, G};

use crate::crypto::random_scalar;
use crate::wire::{Reader, Writer};
use crate::Error;

const POK_TAG: &str = "ob-escrow/pok/v1";
const DLEQ_TAG: &str = "ob-escrow/dleq/v1";
const BIT_TAG: &str = "ob-escrow/bit/v1";

/// Tagged hash with length-prefixed parts.
pub fn tagged(tag: &str, parts: &[&[u8]]) -> [u8; 32] {
    let tag_hash = sha256::Hash::hash(tag.as_bytes());
    let mut engine = sha256::Hash::engine();
    engine.input(tag_hash.as_byte_array());
    engine.input(tag_hash.as_byte_array());
    for part in parts {
        engine.input(&(part.len() as u32).to_le_bytes());
        engine.input(part);
    }
    sha256::Hash::from_engine(engine).to_byte_array()
}

fn challenge(tag: &str, parts: &[&[u8]]) -> MaybeScalar {
    MaybeScalar::reduce_from(&tagged(tag, parts))
}

fn ser(point: &MaybePoint) -> [u8; 33] {
    point.serialize()
}

/// Schnorr proof of knowledge of a discrete log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pok {
    c: MaybeScalar,
    z: MaybeScalar,
}

impl Pok {
    pub fn prove(secret: Scalar, ctx: &[u8; 32]) -> Self {
        let point = secret * G;
        let k = random_scalar();
        let commit = MaybePoint::from(k * G);
        let c = challenge(POK_TAG, &[ctx, &point.serialize(), &ser(&commit)]);
        Self {
            c,
            z: k + c * secret,
        }
    }

    pub fn verify(&self, point: &Point, ctx: &[u8; 32]) -> Result<(), Error> {
        let commit = self.z * G - self.c * *point;
        let c = challenge(POK_TAG, &[ctx, &point.serialize(), &ser(&commit)]);
        if commit.is_infinity() || c != self.c {
            return Err(Error::BadProof);
        }
        Ok(())
    }

    pub fn write(&self, w: &mut Writer) {
        w.scalar(&self.c);
        w.scalar(&self.z);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            c: r.scalar()?,
            z: r.scalar()?,
        })
    }
}

/// Chaum-Pedersen proof that `A = xG` and `B = xH` for the same `x`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dleq {
    c: MaybeScalar,
    z: MaybeScalar,
}

impl Dleq {
    pub fn prove(secret: Scalar, h: &Point, a: &Point, b: &Point, ctx: &[u8; 32]) -> Self {
        let k = random_scalar();
        let k1 = MaybePoint::from(k * G);
        let k2 = MaybePoint::from(k * *h);
        let c = Self::challenge(ctx, h, a, b, &k1, &k2);
        Self {
            c,
            z: k + c * secret,
        }
    }

    pub fn verify(&self, h: &Point, a: &Point, b: &Point, ctx: &[u8; 32]) -> Result<(), Error> {
        let k1 = self.z * G - self.c * *a;
        let k2 = self.z * *h - self.c * *b;
        if k1.is_infinity() || k2.is_infinity() || Self::challenge(ctx, h, a, b, &k1, &k2) != self.c
        {
            return Err(Error::BadProof);
        }
        Ok(())
    }

    fn challenge(
        ctx: &[u8; 32],
        h: &Point,
        a: &Point,
        b: &Point,
        k1: &MaybePoint,
        k2: &MaybePoint,
    ) -> MaybeScalar {
        challenge(
            DLEQ_TAG,
            &[
                ctx,
                &h.serialize(),
                &a.serialize(),
                &b.serialize(),
                &ser(k1),
                &ser(k2),
            ],
        )
    }

    pub fn write(&self, w: &mut Writer) {
        w.scalar(&self.c);
        w.scalar(&self.z);
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            c: r.scalar()?,
            z: r.scalar()?,
        })
    }
}

/// Proof that `(A, B) = (rG, rM + bG)` for some `r` and a bit `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitProof {
    c: [MaybeScalar; 2],
    z: [MaybeScalar; 2],
}

impl BitProof {
    pub fn prove(
        bit: bool,
        r: Scalar,
        m: &Point,
        a: &Point,
        b: &Point,
        ctx: &[u8; 32],
        index: u32,
    ) -> Self {
        let targets = Self::targets(b);
        let real = bit as usize;
        let fake = 1 - real;
        let mut c = [MaybeScalar::Zero; 2];
        let mut z = [MaybeScalar::Zero; 2];
        let mut k1 = [MaybePoint::Infinity; 2];
        let mut k2 = [MaybePoint::Infinity; 2];

        c[fake] = MaybeScalar::from(random_scalar());
        z[fake] = MaybeScalar::from(random_scalar());
        k1[fake] = z[fake] * G - c[fake] * *a;
        k2[fake] = z[fake] * *m - c[fake] * targets[fake];

        let k = random_scalar();
        k1[real] = MaybePoint::from(k * G);
        k2[real] = MaybePoint::from(k * *m);

        let total = Self::challenge(ctx, index, m, a, b, &k1, &k2);
        c[real] = total - c[fake];
        z[real] = k + c[real] * r;
        Self { c, z }
    }

    pub fn verify(
        &self,
        m: &Point,
        a: &Point,
        b: &Point,
        ctx: &[u8; 32],
        index: u32,
    ) -> Result<(), Error> {
        let targets = Self::targets(b);
        let mut k1 = [MaybePoint::Infinity; 2];
        let mut k2 = [MaybePoint::Infinity; 2];
        for j in 0..2 {
            k1[j] = self.z[j] * G - self.c[j] * *a;
            k2[j] = self.z[j] * *m - self.c[j] * targets[j];
        }
        let total = Self::challenge(ctx, index, m, a, b, &k1, &k2);
        if total != self.c[0] + self.c[1] {
            return Err(Error::BadProof);
        }
        Ok(())
    }

    fn targets(b: &Point) -> [MaybePoint; 2] {
        [MaybePoint::from(*b), *b - G]
    }

    fn challenge(
        ctx: &[u8; 32],
        index: u32,
        m: &Point,
        a: &Point,
        b: &Point,
        k1: &[MaybePoint; 2],
        k2: &[MaybePoint; 2],
    ) -> MaybeScalar {
        challenge(
            BIT_TAG,
            &[
                ctx,
                &index.to_le_bytes(),
                &m.serialize(),
                &a.serialize(),
                &b.serialize(),
                &ser(&k1[0]),
                &ser(&k2[0]),
                &ser(&k1[1]),
                &ser(&k2[1]),
            ],
        )
    }

    pub fn write(&self, w: &mut Writer) {
        for scalar in self.c.iter().chain(self.z.iter()) {
            w.scalar(scalar);
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            c: [r.scalar()?, r.scalar()?],
            z: [r.scalar()?, r.scalar()?],
        })
    }
}
