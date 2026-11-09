//! State both parties derive identically from the signed terms and the outpoint.

use bitcoin::{OutPoint, Transaction, TxOut};
use musig2::secp::{Point, Scalar};
use musig2::{AdaptorSignature, KeyAggContext, LiftedSignature};

use crate::context::Context;
use crate::crypto;
use crate::proofs::tagged;
use crate::terms::SignedTerms;
use crate::tx::{self, Spend, Templates};
use crate::vencrypt::VerifiableShare;
use crate::wire::{Reader, Writer};
use crate::{Error, Party, Phase};

#[derive(Clone, Debug)]
pub(crate) struct Contract {
    pub signed: SignedTerms,
    pub outpoint: OutPoint,
    pub funding_output: TxOut,
    pub key_agg: KeyAggContext,
    pub output_key: [u8; 32],
    pub buyer: Point,
    pub vendor: Point,
    /// Moderator encryption key: shares are encrypted to it.
    pub moderator: Point,
    pub templates: Templates,
}

/// Who is constructing the contract, which decides the height check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Buyer or vendor at funding: `ctx.tip` is the current tip.
    Party,
    /// Moderator reviewing a letter: `ctx.tip` is the height it arrived.
    Moderator,
    /// A party reloading its own stored state: the heights were checked at
    /// funding and are not checked against `ctx.tip` again.
    Restore,
}

impl Contract {
    /// The one place every term is checked, then every spend is derived.
    ///
    /// The terms must name exactly `ctx.moderator`, satisfy both `ctx.policy`
    /// and the moderator's published policy, and (for a party) fit the
    /// timeline relative to `ctx.tip`. A moderator only checks that the
    /// letter arrived by `dispute_deadline`.
    pub fn new(
        signed: SignedTerms,
        outpoint: OutPoint,
        ctx: &Context,
        role: Role,
    ) -> Result<Self, Error> {
        let terms = signed.terms();
        if terms.moderator != ctx.moderator {
            return Err(Error::ModeratorMismatch);
        }
        terms.validate(&ctx.policy)?;
        terms.validate(&terms.moderator.policy)?;
        match role {
            Role::Party => {
                terms.validate_at_tip(&ctx.policy, ctx.tip)?;
                terms.validate_at_tip(&terms.moderator.policy, ctx.tip)?;
            }
            Role::Moderator if ctx.tip > terms.dispute_deadline => {
                return Err(Error::DisputeWindowClosed);
            }
            Role::Moderator | Role::Restore => {}
        }
        let buyer = terms.key(Party::Buyer)?;
        let vendor = terms.key(Party::Vendor)?;
        let moderator = terms.moderator.encryption_point()?;
        let key_agg = crypto::aggregate(buyer, vendor)?;
        let output_key = crypto::output_xonly(&key_agg);
        let funding_output = terms.funding_output()?;
        let templates = Templates::build(terms, outpoint)?;
        Ok(Self {
            signed,
            outpoint,
            funding_output,
            key_agg,
            output_key,
            buyer,
            vendor,
            moderator,
            templates,
        })
    }

    pub fn tx(&self, spend: Spend) -> &Transaction {
        self.templates.get(spend)
    }

    pub fn sighash(&self, spend: Spend) -> Result<[u8; 32], Error> {
        tx::key_spend_sighash(self.tx(spend), &self.funding_output)
    }

    pub fn key(&self, party: Party) -> Point {
        match party {
            Party::Buyer => self.buyer,
            Party::Vendor => self.vendor,
        }
    }

    /// Fiat-Shamir context for one dispute branch.
    pub fn branch_context(&self, favored: Party) -> Result<[u8; 32], Error> {
        let sighash = self.sighash(Spend::dispute(favored))?;
        let mut outpoint = Writer::new(b"OBBC", 1);
        outpoint.outpoint(&self.outpoint);
        Ok(tagged(
            "ob-escrow/branch/v1",
            &[
                &[favored as u8],
                &sighash,
                outpoint.as_slice(),
                &self.signed.terms().digest(),
            ],
        ))
    }

    /// BIP 341 key-spend check against the funding output. Does not apply `nLockTime`.
    pub fn verify_key_spend(&self, tx: &Transaction) -> Result<(), Error> {
        if tx.input.len() != 1 || tx.input[0].previous_output != self.outpoint {
            return Err(Error::BadSignature);
        }
        verify_spend(tx, &self.funding_output, &self.output_key)
    }

    /// Aggregate two plain partials into a fully signed, verified spend.
    pub fn finish_plain(
        &self,
        spend: Spend,
        nonce: &musig2::AggNonce,
        partials: [musig2::PartialSignature; 2],
    ) -> Result<Transaction, Error> {
        let sighash = self.sighash(spend)?;
        let signature: LiftedSignature =
            crypto::aggregate_plain(&self.key_agg, nonce, partials, &sighash)?;
        let tx = tx::attach_key_spend(self.tx(spend).clone(), &signature.serialize());
        self.verify_key_spend(&tx)?;
        Ok(tx)
    }
}

fn verify_spend(tx: &Transaction, funding: &TxOut, output_key: &[u8; 32]) -> Result<(), Error> {
    let committed = tx::p2tr_output_key(&funding.script_pubkey).ok_or(Error::KeyMismatch)?;
    if &committed != output_key {
        return Err(Error::KeyMismatch);
    }
    let signature = tx::witness_signature(tx)?;
    let sighash = tx::key_spend_sighash(tx, funding)?;
    crypto::bip340_verify(output_key, &signature, &sighash)
}

/// One dispute outcome as its favored party holds it.
#[derive(Clone, Debug)]
pub(crate) struct Branch {
    pub favored: Party,
    /// `T_fav`: the favored party's half, derived from its per-order key.
    pub party_point: Point,
    /// The loser-created moderator half, verifiably encrypted to the moderator.
    pub share: VerifiableShare,
    pub adaptor_sig: AdaptorSignature,
}

impl Branch {
    pub fn adaptor_point(&self) -> Result<Point, Error> {
        adaptor_point(self.party_point, self.share.point())
    }
}

pub(crate) fn adaptor_point(party: Point, moderator: Point) -> Result<Point, Error> {
    (party + moderator).not_inf().map_err(|_| Error::BadProof)
}

pub(crate) fn branch_label(favored: Party) -> &'static [u8] {
    match favored {
        Party::Buyer => b"buyer-wins",
        Party::Vendor => b"vendor-wins",
    }
}

pub(crate) fn party_half(
    secret: &Scalar,
    contract: &Contract,
    favored: Party,
) -> Result<Scalar, Error> {
    let sighash = contract.sighash(Spend::dispute(favored))?;
    crypto::party_adaptor_secret(secret, branch_label(favored), &sighash)
}

pub(crate) fn enter(phase: &mut Phase, next: Phase) -> Result<(), Error> {
    if *phase != Phase::Funded || next == Phase::Funded {
        return Err(Error::InvalidTransition {
            from: *phase,
            to: next,
        });
    }
    *phase = next;
    Ok(())
}

const PRESIGNED_MAGIC: &[u8; 4] = b"OBPS";
const PRESIGNED_VERSION: u8 = 1;

/// A fully signed, timelocked spend that can be stored and broadcast later.
///
/// Loading from bytes re-verifies the signature against the funding output
/// and the aggregate key, so a corrupted or swapped record is rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Presigned {
    tx: Transaction,
    funding_output: TxOut,
    output_key: [u8; 32],
}

impl Presigned {
    fn new(tx: Transaction, contract: &Contract) -> Result<Self, Error> {
        contract.verify_key_spend(&tx)?;
        Ok(Self {
            tx,
            funding_output: contract.funding_output.clone(),
            output_key: contract.output_key,
        })
    }

    fn broadcastable_at(&self, height: u32) -> Result<Transaction, Error> {
        tx::locktime_error(&self.tx, height)?;
        Ok(self.tx.clone())
    }

    fn to_bytes(&self, kind: u8) -> Vec<u8> {
        let mut w = Writer::new(PRESIGNED_MAGIC, PRESIGNED_VERSION);
        w.u8(kind);
        w.bytes(&self.output_key);
        w.amount(self.funding_output.value);
        w.script(&self.funding_output.script_pubkey);
        w.tx(&self.tx);
        w.finish()
    }

    fn from_bytes(bytes: &[u8], kind: u8) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, PRESIGNED_MAGIC, PRESIGNED_VERSION)?;
        if r.u8()? != kind {
            return Err(Error::Malformed);
        }
        let output_key = r.array()?;
        let funding_output = TxOut {
            value: r.amount()?,
            script_pubkey: r.script()?,
        };
        let tx = r.tx()?;
        r.finish()?;
        if tx.input.len() != 1 || tx.lock_time == bitcoin::absolute::LockTime::ZERO {
            return Err(Error::Malformed);
        }
        verify_spend(&tx, &funding_output, &output_key)?;
        Ok(Self {
            tx,
            funding_output,
            output_key,
        })
    }
}

macro_rules! presigned_type {
    ($(#[$doc:meta])* $name:ident, $kind:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name(Presigned);

        impl $name {
            pub(crate) fn new(tx: Transaction, contract: &Contract) -> Result<Self, Error> {
                Presigned::new(tx, contract).map(Self)
            }

            /// The signed transaction. Consensus rejects it before its locktime.
            pub fn transaction(&self) -> &Transaction {
                &self.0.tx
            }

            pub fn funding_output(&self) -> &TxOut {
                &self.0.funding_output
            }

            /// The transaction if `height` (the next block's height) passes `nLockTime`.
            pub fn broadcastable_at(&self, height: u32) -> Result<Transaction, Error> {
                self.0.broadcastable_at(height)
            }

            pub fn to_bytes(&self) -> Vec<u8> {
                self.0.to_bytes($kind)
            }

            /// Parse and re-verify a stored record.
            pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
                Presigned::from_bytes(bytes, $kind).map(Self)
            }
        }
    };
}

presigned_type!(
    /// The buyer's refund: pays the buyer at `refund_height`. Fully signed
    /// and verified against the aggregate key and the exact funding outpoint
    /// and value before the buyer can see the funding PSBT.
    SignedRefund,
    1
);

presigned_type!(
    /// The presigned claim. It pays the buyer at `refund_height`, so a silent
    /// moderator cannot pay the vendor.
    SignedClaim,
    2
);
