//! Escrow spend templates, fee checks, and the consensus locktime rule.
//!
//! No network. Signature hashes are BIP341 key-spend hashes with
//! `SIGHASH_DEFAULT`.

use bitcoin::absolute::LOCK_TIME_THRESHOLD;
use bitcoin::hashes::Hash;
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::{absolute, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};

use crate::terms::{EscrowTerms, Policy};
use crate::{Error, Party};

/// Absolute locktime is enabled and replace-by-fee is not.
///
/// This is `0xfffffffe`: not `0xffffffff`, so Bitcoin Core honors `nLockTime`.
pub const LOCKTIME_SEQUENCE: Sequence = Sequence::ENABLE_LOCKTIME_NO_RBF;

/// Bitcoin Core `IsFinalTx` for a height the caller already knows.
///
/// Height-based `nLockTime` is the last height at which the transaction is
/// **not** final. It can be mined in a block of height `nLockTime + 1`.
/// Time-based locktimes need a median time past, which this function does not
/// have; those transactions are reported as not final.
pub fn is_final_at_height(tx: &Transaction, height: u32) -> bool {
    let lock = tx.lock_time.to_consensus_u32();
    if lock == 0 {
        return true;
    }
    if tx.input.iter().all(|input| input.sequence == Sequence::MAX) {
        return true;
    }
    if lock < LOCK_TIME_THRESHOLD {
        lock < height
    } else {
        false
    }
}

pub fn locktime_error(tx: &Transaction, height: u32) -> Result<(), Error> {
    if is_final_at_height(tx, height) {
        Ok(())
    } else {
        Err(Error::LockTimeNotSatisfied {
            lock_time: tx.lock_time.to_consensus_u32(),
            height,
        })
    }
}

pub fn payout_script(internal_xonly: &[u8; 32]) -> Result<ScriptBuf, Error> {
    let secp = bitcoin::secp256k1::Secp256k1::verification_only();
    let key = bitcoin::secp256k1::XOnlyPublicKey::from_slice(internal_xonly)
        .map_err(|_| Error::InvalidParams("x-only key"))?;
    Ok(ScriptBuf::new_p2tr(&secp, key, None))
}

pub fn p2tr_output_key(script: &ScriptBuf) -> Option<[u8; 32]> {
    let bytes = script.as_bytes();
    if bytes.len() == 34 && bytes[0] == 0x51 && bytes[1] == 0x20 {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[2..]);
        Some(key)
    } else {
        None
    }
}

pub fn funding_script(
    internal_xonly: &[u8; 32],
    musig_output_xonly: &[u8; 32],
) -> Result<ScriptBuf, Error> {
    let script = payout_script(internal_xonly)?;
    match p2tr_output_key(&script) {
        Some(key) if &key == musig_output_xonly => Ok(script),
        _ => Err(Error::KeyMismatch),
    }
}

/// One input spending `outpoint`, paying `outputs` in order.
pub fn spend_template(
    outpoint: OutPoint,
    lock_time: absolute::LockTime,
    outputs: Vec<TxOut>,
) -> Transaction {
    Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: LOCKTIME_SEQUENCE,
            witness: Witness::new(),
        }],
        output: outputs,
    }
}

/// Which presigned or cooperative spend of the funding output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spend {
    /// Happy path, signed when the buyer is satisfied. Pays the vendor.
    Release,
    /// Buyer backstop at `refund_height`. Pays the buyer.
    Refund,
    /// Vendor default at `vendor_claim_height`. Pays the vendor.
    Claim,
    /// Dispute outcome paying the buyer and the moderator fee.
    BuyerWins,
    /// Dispute outcome paying the vendor and the moderator fee.
    VendorWins,
}

impl Spend {
    pub fn dispute(favored: Party) -> Self {
        match favored {
            Party::Buyer => Spend::BuyerWins,
            Party::Vendor => Spend::VendorWins,
        }
    }
}

/// Every transaction the escrow can sign, derived only from the terms and outpoint.
#[derive(Clone, Debug)]
pub struct Templates {
    pub release: Transaction,
    pub refund: Transaction,
    pub claim: Transaction,
    pub buyer_wins: Transaction,
    pub vendor_wins: Transaction,
}

impl Templates {
    pub fn build(terms: &EscrowTerms, outpoint: OutPoint) -> Result<Self, Error> {
        let payout = terms
            .funding_value
            .checked_sub(terms.network_fee)
            .ok_or(Error::InvalidParams("fee exceeds the funding value"))?;
        let winner = payout
            .checked_sub(terms.moderator_fee)
            .ok_or(Error::InvalidParams("moderator fee exceeds the payout"))?;
        let single = |party: Party, lock: absolute::LockTime| {
            spend_template(
                outpoint,
                lock,
                vec![TxOut {
                    value: payout,
                    script_pubkey: terms.payout(party).clone(),
                }],
            )
        };
        let dispute = |party: Party| {
            spend_template(
                outpoint,
                absolute::LockTime::ZERO,
                vec![
                    TxOut {
                        value: winner,
                        script_pubkey: terms.payout(party).clone(),
                    },
                    TxOut {
                        value: terms.moderator_fee,
                        script_pubkey: terms.moderator_payout.clone(),
                    },
                ],
            )
        };
        Ok(Self {
            release: single(Party::Vendor, absolute::LockTime::ZERO),
            refund: single(Party::Buyer, height_lock(terms.refund_height)?),
            claim: single(Party::Vendor, height_lock(terms.vendor_claim_height)?),
            buyer_wins: dispute(Party::Buyer),
            vendor_wins: dispute(Party::Vendor),
        })
    }

    pub fn get(&self, spend: Spend) -> &Transaction {
        match spend {
            Spend::Release => &self.release,
            Spend::Refund => &self.refund,
            Spend::Claim => &self.claim,
            Spend::BuyerWins => &self.buyer_wins,
            Spend::VendorWins => &self.vendor_wins,
        }
    }

    pub fn all(&self) -> [&Transaction; 5] {
        [
            &self.release,
            &self.refund,
            &self.claim,
            &self.buyer_wins,
            &self.vendor_wins,
        ]
    }
}

fn height_lock(height: u32) -> Result<absolute::LockTime, Error> {
    absolute::LockTime::from_height(height).map_err(|_| Error::InvalidParams("lock height"))
}

/// Reject a spend whose fee is zero or below `policy.min_fee_rate_sat_vb`.
///
/// The vsize is measured with the 64-byte key-path witness attached, which is
/// exactly what the signed transaction carries.
pub fn check_fee_rate(tx: &Transaction, input: Amount, policy: &Policy) -> Result<(), Error> {
    let spent: Amount = tx.output.iter().map(|out| out.value).sum();
    let fee = input.checked_sub(spent).ok_or(Error::FeeTooLow)?;
    let signed = attach_key_spend(tx.clone(), &[0u8; 64]);
    let floor = policy
        .min_fee_rate_sat_vb
        .max(1)
        .saturating_mul(signed.vsize() as u64);
    if fee.to_sat() == 0 || fee.to_sat() < floor {
        return Err(Error::FeeTooLow);
    }
    Ok(())
}

pub fn key_spend_sighash(tx: &Transaction, funding_output: &TxOut) -> Result<[u8; 32], Error> {
    if tx.input.len() != 1 {
        return Err(Error::BadSignature);
    }
    let prevouts = Prevouts::All(&[funding_output]);
    let mut cache = SighashCache::new(tx);
    let sighash = cache
        .taproot_key_spend_signature_hash(0, &prevouts, TapSighashType::Default)
        .map_err(|_| Error::BadSignature)?;
    Ok(*sighash.as_byte_array())
}

pub fn attach_key_spend(mut tx: Transaction, signature: &[u8; 64]) -> Transaction {
    let mut witness = Witness::new();
    witness.push(signature);
    tx.input[0].witness = witness;
    tx
}

pub fn witness_signature(tx: &Transaction) -> Result<[u8; 64], Error> {
    let witness = tx
        .input
        .first()
        .map(|input| &input.witness)
        .ok_or(Error::BadSignature)?;
    if witness.len() != 1 {
        return Err(Error::BadSignature);
    }
    let item = witness.nth(0).ok_or(Error::BadSignature)?;
    item.try_into().map_err(|_| Error::BadSignature)
}
