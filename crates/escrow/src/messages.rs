//! Messages the buyer and vendor exchange during funding and release.
//!
//! None of these carry a secret key or a secret nonce. Each round's message
//! is produced by one party's state machine and consumed by the other's.

use bitcoin::OutPoint;
use musig2::secp::Point;
use musig2::{PartialSignature, PubNonce};

use crate::proofs::Pok;
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer};
use crate::Error;

const VERSION: u8 = 1;

/// Public nonces for the four spends signed at funding, in this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingNonces {
    pub refund: PubNonce,
    pub claim: PubNonce,
    pub buyer_wins: PubNonce,
    pub vendor_wins: PubNonce,
}

impl FundingNonces {
    pub(crate) fn write(&self, w: &mut Writer) {
        for nonce in [
            &self.refund,
            &self.claim,
            &self.buyer_wins,
            &self.vendor_wins,
        ] {
            w.nonce(nonce);
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            refund: r.nonce()?,
            claim: r.nonce()?,
            buyer_wins: r.nonce()?,
            vendor_wins: r.nonce()?,
        })
    }
}

/// Round 1, buyer to vendor.
///
/// The buyer would lose the vendor-wins branch, so it creates that branch's
/// moderator half and proves the ciphertext decrypts to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuyerOpen {
    pub funding_outpoint: OutPoint,
    pub nonces: FundingNonces,
    /// `T_b`, the buyer's half of the buyer-wins adaptor point.
    pub buyer_half: Point,
    pub buyer_half_proof: Pok,
    pub vendor_wins_share: VerifiableShare,
}

/// Round 2, vendor to buyer.
///
/// Carries the vendor's partials on the two spends that protect the buyer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VendorReply {
    pub nonces: FundingNonces,
    /// `T_v`, the vendor's half of the vendor-wins adaptor point.
    pub vendor_half: Point,
    pub vendor_half_proof: Pok,
    pub buyer_wins_share: VerifiableShare,
    pub refund_partial: PartialSignature,
    pub buyer_wins_partial: PartialSignature,
}

/// Round 3, buyer to vendor: partials on the two spends that protect the vendor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuyerSignatures {
    pub claim_partial: PartialSignature,
    pub vendor_wins_partial: PartialSignature,
}

/// Happy-path release, buyer to vendor: the buyer's nonce and partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSignature {
    pub nonce: PubNonce,
    pub partial: PartialSignature,
}

impl BuyerOpen {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(b"OBR1", VERSION);
        w.outpoint(&self.funding_outpoint);
        self.nonces.write(&mut w);
        w.point(&self.buyer_half);
        self.buyer_half_proof.write(&mut w);
        self.vendor_wins_share.write(&mut w);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, b"OBR1", VERSION)?;
        let message = Self {
            funding_outpoint: r.outpoint()?,
            nonces: FundingNonces::read(&mut r)?,
            buyer_half: r.point()?,
            buyer_half_proof: Pok::read(&mut r)?,
            vendor_wins_share: VerifiableShare::read(&mut r)?,
        };
        r.finish()?;
        Ok(message)
    }
}

impl VendorReply {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(b"OBR2", VERSION);
        self.nonces.write(&mut w);
        w.point(&self.vendor_half);
        self.vendor_half_proof.write(&mut w);
        self.buyer_wins_share.write(&mut w);
        w.partial(&self.refund_partial);
        w.partial(&self.buyer_wins_partial);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, b"OBR2", VERSION)?;
        let message = Self {
            nonces: FundingNonces::read(&mut r)?,
            vendor_half: r.point()?,
            vendor_half_proof: Pok::read(&mut r)?,
            buyer_wins_share: VerifiableShare::read(&mut r)?,
            refund_partial: r.partial()?,
            buyer_wins_partial: r.partial()?,
        };
        r.finish()?;
        Ok(message)
    }
}

impl BuyerSignatures {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(b"OBR3", VERSION);
        w.partial(&self.claim_partial);
        w.partial(&self.vendor_wins_partial);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, b"OBR3", VERSION)?;
        let message = Self {
            claim_partial: r.partial()?,
            vendor_wins_partial: r.partial()?,
        };
        r.finish()?;
        Ok(message)
    }
}

impl ReleaseSignature {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new(b"OBRL", VERSION);
        w.nonce(&self.nonce);
        w.partial(&self.partial);
        w.finish()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, b"OBRL", VERSION)?;
        let message = Self {
            nonce: r.nonce()?,
            partial: r.partial()?,
        };
        r.finish()?;
        Ok(message)
    }
}
