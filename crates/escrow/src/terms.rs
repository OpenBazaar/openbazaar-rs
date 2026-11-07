//! Escrow terms both parties sign before any funding round.
//!
//! The terms fix every payout the escrow can make: who is paid, how much, at
//! which height, and which moderator (with which fee, payout script, and
//! policy) may pick between the two dispute outcomes. Nothing outside the
//! signed terms can appear in any presigned transaction.

use bitcoin::absolute::LOCK_TIME_THRESHOLD;
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut};
use musig2::secp::{Point, Scalar, G};

use crate::crypto::{self, point_from_compressed, scalar_key};
use crate::moderator::ModeratorRecord;
use crate::policy::{check_payout, Policy, ANCHOR_VALUE};
use crate::proofs::tagged;
use crate::tx::{self, Templates};
use crate::wire::{Reader, Writer, SCRIPT_MAX};
use crate::{Error, Party};

const TERMS_MAGIC: &[u8; 4] = b"OBET";
/// Not `OBST`, which is the catalog store record.
const SIGNED_MAGIC: &[u8; 4] = b"OBTS";
/// v3 is the M3 claim: the vendor claim pays the moderator fee. v2 signed a
/// fee-free claim, so a v2 encoding must not decode.
const VERSION: u8 = 3;

/// Everything the escrow can pay, agreed before funding.
///
/// Heights are absolute block heights used as `nLockTime`. A transaction with
/// `nLockTime = h` can first be mined in block `h + 1`. The required order is
/// `tip + delivery window <= dispute_deadline < vendor_claim_height <
/// refund_height <= tip + refund horizon`; see the crate docs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowTerms {
    /// Buyer per-order key (compressed). See [`order_public_key`].
    pub buyer: [u8; 33],
    /// Vendor per-order key (compressed).
    pub vendor: [u8; 33],
    /// The moderator's published record, verbatim. Never aggregated.
    pub moderator: ModeratorRecord,
    pub funding_value: Amount,
    /// Network fee carried by every presigned transaction.
    pub network_fee: Amount,
    /// Last height at which the moderator accepts a dispute letter.
    pub dispute_deadline: u32,
    /// `nLockTime` of the vendor claim (the vendor default).
    pub vendor_claim_height: u32,
    /// `nLockTime` of the buyer refund (the vendor-vanished backstop).
    pub refund_height: u32,
    pub buyer_payout: ScriptBuf,
    pub vendor_payout: ScriptBuf,
}

impl EscrowTerms {
    /// Structural, standardness, and fee checks under `policy`.
    pub fn validate(&self, policy: &Policy) -> Result<(), Error> {
        self.moderator.validate()?;
        let buyer = point_from_compressed(&self.buyer)?;
        let vendor = point_from_compressed(&self.vendor)?;
        let keys = [
            buyer,
            vendor,
            self.moderator.signing_point()?,
            self.moderator.encryption_point()?,
        ];
        for (i, a) in keys.iter().enumerate() {
            if keys[i + 1..].contains(a) {
                return Err(Error::InvalidParams("escrow keys must all differ"));
            }
        }
        self.validate_timeline(policy)?;
        let scripts = [
            &self.buyer_payout,
            &self.vendor_payout,
            &self.moderator.payout,
        ];
        for (i, script) in scripts.iter().enumerate() {
            if script.len() > SCRIPT_MAX {
                return Err(Error::InvalidParams("payout script is too long"));
            }
            if scripts[i + 1..].contains(script) {
                return Err(Error::InvalidParams("payout scripts must all differ"));
            }
        }
        if self.network_fee == Amount::ZERO {
            return Err(Error::FeeTooLow);
        }
        if self.network_fee > policy.max_network_fee {
            return Err(Error::FeeTooHigh);
        }
        self.funding_value
            .checked_sub(self.network_fee)
            .and_then(|rest| rest.checked_sub(self.moderator.fee))
            .and_then(|rest| rest.checked_sub(ANCHOR_VALUE))
            .ok_or(Error::InvalidParams("fees exceed the funding value"))?;
        let templates = Templates::build(self, OutPoint::null())?;
        for spend in templates.all() {
            for out in &spend.output {
                if out.script_pubkey != tx::anchor_script() {
                    check_payout(&out.script_pubkey, out.value)?;
                }
            }
            tx::check_fee_rate(spend, self.funding_value, policy)?;
        }
        Ok(())
    }

    fn validate_timeline(&self, policy: &Policy) -> Result<(), Error> {
        let heights = [
            self.dispute_deadline,
            self.vendor_claim_height,
            self.refund_height,
        ];
        if heights.iter().any(|h| *h == 0 || *h >= LOCK_TIME_THRESHOLD) {
            return Err(Error::InvalidParams(
                "heights must be non-zero block heights",
            ));
        }
        let ruling = self.vendor_claim_height.checked_sub(self.dispute_deadline);
        let claim = self.refund_height.checked_sub(self.vendor_claim_height);
        match (ruling, claim) {
            (Some(r), Some(c)) if r >= policy.min_ruling_window && c >= policy.min_claim_window => {
                Ok(())
            }
            _ => Err(Error::InvalidParams(
                "need dispute_deadline < vendor_claim_height < refund_height with policy gaps",
            )),
        }
    }

    /// Heights relative to the current chain tip, checked at funding.
    ///
    /// The buyer needs `min_delivery_window` blocks to receive the goods and
    /// dispute, and its coins are locked for at most `max_refund_horizon`.
    pub fn validate_at_tip(&self, policy: &Policy, tip: u32) -> Result<(), Error> {
        let earliest_deadline = tip.checked_add(policy.min_delivery_window);
        let latest_refund = tip.checked_add(policy.max_refund_horizon);
        match (earliest_deadline, latest_refund) {
            (Some(deadline), Some(refund))
                if self.dispute_deadline >= deadline && self.refund_height <= refund =>
            {
                Ok(())
            }
            _ => Err(Error::HeightsOutOfRange),
        }
    }

    pub fn key(&self, party: Party) -> Result<Point, Error> {
        point_from_compressed(match party {
            Party::Buyer => &self.buyer,
            Party::Vendor => &self.vendor,
        })
    }

    pub fn payout(&self, party: Party) -> &ScriptBuf {
        match party {
            Party::Buyer => &self.buyer_payout,
            Party::Vendor => &self.vendor_payout,
        }
    }

    /// Taproot output both parties fund. No moderator key is aggregated.
    pub fn funding_output(&self) -> Result<TxOut, Error> {
        let buyer = point_from_compressed(&self.buyer)?;
        let vendor = point_from_compressed(&self.vendor)?;
        let internal = crypto::internal_xonly(buyer, vendor)?;
        let output_key = crypto::output_xonly(&crypto::aggregate(buyer, vendor)?);
        Ok(TxOut {
            value: self.funding_value,
            script_pubkey: tx::funding_script(&internal, &output_key)?,
        })
    }

    pub fn digest(&self) -> [u8; 32] {
        tagged("ob-escrow/terms/v3", &[&self.encode()])
    }

    /// Sign the terms with a per-order key. The key must be the buyer's or the vendor's.
    pub fn sign(&self, per_order_secret: &[u8; 32]) -> Result<[u8; 64], Error> {
        let secret = scalar_key(per_order_secret)?;
        let point = secret * G;
        if point != self.key(Party::Buyer)? && point != self.key(Party::Vendor)? {
            return Err(Error::WrongKey);
        }
        crypto::schnorr_sign(&secret, &self.digest())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(TERMS_MAGIC, VERSION);
        w.bytes(&self.buyer);
        w.bytes(&self.vendor);
        self.moderator.write(&mut w);
        w.amount(self.funding_value);
        w.amount(self.network_fee);
        w.u32(self.dispute_deadline);
        w.u32(self.vendor_claim_height);
        w.u32(self.refund_height);
        w.script(&self.buyer_payout);
        w.script(&self.vendor_payout);
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, TERMS_MAGIC, VERSION)?;
        let terms = Self {
            buyer: r.array()?,
            vendor: r.array()?,
            moderator: ModeratorRecord::read(&mut r)?,
            funding_value: r.amount()?,
            network_fee: r.amount()?,
            dispute_deadline: r.u32()?,
            vendor_claim_height: r.u32()?,
            refund_height: r.u32()?,
            buyer_payout: r.script()?,
            vendor_payout: r.script()?,
        };
        r.finish()?;
        Ok(terms)
    }
}

/// Terms carrying valid BIP 340 signatures from both per-order keys.
///
/// Constructing one verifies both signatures, so holding a `SignedTerms`
/// means both parties agreed to the moderator's record, fee, and policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedTerms {
    terms: EscrowTerms,
    buyer_signature: [u8; 64],
    vendor_signature: [u8; 64],
}

impl SignedTerms {
    pub fn new(
        terms: EscrowTerms,
        buyer_signature: [u8; 64],
        vendor_signature: [u8; 64],
    ) -> Result<Self, Error> {
        let digest = terms.digest();
        crypto::schnorr_verify(&terms.key(Party::Buyer)?, &digest, &buyer_signature)
            .map_err(|_| Error::BadTermsSignature)?;
        crypto::schnorr_verify(&terms.key(Party::Vendor)?, &digest, &vendor_signature)
            .map_err(|_| Error::BadTermsSignature)?;
        Ok(Self {
            terms,
            buyer_signature,
            vendor_signature,
        })
    }

    pub fn terms(&self) -> &EscrowTerms {
        &self.terms
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(SIGNED_MAGIC, VERSION);
        w.var(&self.terms.encode());
        w.bytes(&self.buyer_signature);
        w.bytes(&self.vendor_signature);
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, SIGNED_MAGIC, VERSION)?;
        let terms = EscrowTerms::decode(r.var(usize::MAX)?)?;
        let buyer_signature = r.array()?;
        let vendor_signature = r.array()?;
        r.finish()?;
        Self::new(terms, buyer_signature, vendor_signature)
    }
}

fn order_tweak(long_term_public: &[u8; 33], order_secret: &[u8; 32]) -> Result<Scalar, Error> {
    Scalar::from_slice(&tagged(
        "ob-escrow/order-key/v1",
        &[long_term_public, order_secret],
    ))
    .map_err(|_| Error::InvalidParams("order tweak"))
}

/// Per-order secret key: `x + H(X, order_secret)`.
///
/// `order_secret` is 32 random bytes the buyer and vendor share privately for
/// one order. Without it, two orders between the same pair have unrelated
/// funding keys, and a moderator who sees one dispute cannot find the others.
pub fn order_secret_key(
    long_term_secret: &[u8; 32],
    order_secret: &[u8; 32],
) -> Result<[u8; 32], Error> {
    let secret = scalar_key(long_term_secret)?;
    let tweak = order_tweak(&(secret * G).serialize(), order_secret)?;
    let tweaked = (secret + tweak)
        .not_zero()
        .map_err(|_| Error::InvalidParams("order key"))?;
    Ok(tweaked.serialize())
}

/// Per-order public key matching [`order_secret_key`]: `X + H(X, order_secret)·G`.
pub fn order_public_key(
    long_term_public: &[u8; 33],
    order_secret: &[u8; 32],
) -> Result<[u8; 33], Error> {
    let point = point_from_compressed(long_term_public)?;
    let tweak = order_tweak(long_term_public, order_secret)?;
    let tweaked = (point + tweak * G)
        .not_inf()
        .map_err(|_| Error::InvalidParams("order key"))?;
    Ok(tweaked.serialize())
}
