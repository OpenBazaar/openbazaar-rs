//! Regtest-shaped taproot transactions and the consensus locktime rule.
//!
//! No network. Signature hashes are BIP341 key-spend hashes with
//! `SIGHASH_DEFAULT`.

use bitcoin::absolute::LOCK_TIME_THRESHOLD;
use bitcoin::hashes::Hash;
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::{absolute, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};

use crate::Error;

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

/// One input spending `outpoint`, one output paying `payout`.
pub fn spend_template(
    outpoint: OutPoint,
    lock_time: absolute::LockTime,
    payout_script: ScriptBuf,
    payout: Amount,
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
        output: vec![TxOut {
            value: payout,
            script_pubkey: payout_script,
        }],
    }
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
