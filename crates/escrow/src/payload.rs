//! Dispute letter bytes.
//!
//! The moderator is not a participant at funding time. The letter is the first
//! thing they see: the outpoint, the claimant's memo, the settlement terms,
//! and the encrypted adaptor half.

use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction};

use crate::crypto::{EncryptedSecret, MacContext, MEMO_MAX, SCRIPT_MAX};
use crate::Error;
use crate::Party;

const MAGIC: &[u8; 4] = b"OBDP";
const VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub favored: Party,
    pub memo: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Letter {
    pub outpoint: OutPoint,
    pub claim: Claim,
    pub adaptor_point: [u8; 33],
    pub adaptor_signature: [u8; 65],
    pub encrypted: EncryptedSecret,
    pub sighash: [u8; 32],
    pub lock_time: u32,
    pub value: Amount,
    pub script: ScriptBuf,
}

impl Letter {
    pub fn context(&self) -> MacContext<'_> {
        MacContext {
            adaptor_point: &self.adaptor_point,
            sighash: &self.sighash,
            txid: self.outpoint.txid.as_byte_array(),
            vout: self.outpoint.vout,
            lock_time: self.lock_time,
            value: self.value.to_sat(),
            favored: self.claim.favored as u8,
            script: self.script.as_bytes(),
        }
    }
}

pub fn build(letter: &Letter) -> Result<Vec<u8>, Error> {
    if letter.claim.memo.len() > MEMO_MAX {
        return Err(Error::InvalidParams("claim memo is too long"));
    }
    if letter.script.len() > SCRIPT_MAX {
        return Err(Error::InvalidParams("payout script is too long"));
    }
    let mut out = Vec::with_capacity(320 + letter.claim.memo.len() + letter.script.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(letter.outpoint.txid.as_byte_array());
    out.extend_from_slice(&letter.outpoint.vout.to_le_bytes());
    out.push(letter.claim.favored as u8);
    out.extend_from_slice(&(letter.claim.memo.len() as u32).to_le_bytes());
    out.extend_from_slice(&letter.claim.memo);
    out.extend_from_slice(&letter.adaptor_point);
    out.extend_from_slice(&letter.adaptor_signature);
    out.extend_from_slice(&letter.encrypted.ephemeral_pk);
    out.extend_from_slice(&letter.encrypted.ciphertext);
    out.extend_from_slice(&letter.encrypted.mac);
    out.extend_from_slice(&letter.sighash);
    out.extend_from_slice(&letter.lock_time.to_le_bytes());
    out.extend_from_slice(&letter.value.to_sat().to_le_bytes());
    out.extend_from_slice(&(letter.script.len() as u32).to_le_bytes());
    out.extend_from_slice(letter.script.as_bytes());
    Ok(out)
}

pub fn parse(bytes: &[u8]) -> Result<Letter, Error> {
    let mut cursor = Cursor::new(bytes);
    let magic = cursor.array::<4>()?;
    if &magic != MAGIC || cursor.u8()? != VERSION {
        return Err(Error::BadPayload);
    }
    let txid_bytes = cursor.array::<32>()?;
    let vout = cursor.u32()?;
    let favored = Party::from_byte(cursor.u8()?).ok_or(Error::BadPayload)?;
    let memo_len = cursor.u32()? as usize;
    if memo_len > MEMO_MAX {
        return Err(Error::BadPayload);
    }
    let memo = cursor.bytes(memo_len)?.to_vec();
    let adaptor_point = cursor.array::<33>()?;
    let adaptor_signature = cursor.array::<65>()?;
    let encrypted = EncryptedSecret {
        ephemeral_pk: cursor.array::<33>()?,
        ciphertext: cursor.array::<32>()?,
        mac: cursor.array::<32>()?,
    };
    let sighash = cursor.array::<32>()?;
    let lock_time = cursor.u32()?;
    let value = Amount::from_sat(cursor.u64()?);
    let script_len = cursor.u32()? as usize;
    if script_len > SCRIPT_MAX {
        return Err(Error::BadPayload);
    }
    let script = ScriptBuf::from_bytes(cursor.bytes(script_len)?.to_vec());
    if !cursor.is_empty() {
        return Err(Error::BadPayload);
    }
    Ok(Letter {
        outpoint: OutPoint {
            txid: bitcoin::Txid::from_byte_array(txid_bytes),
            vout,
        },
        claim: Claim { favored, memo },
        adaptor_point,
        adaptor_signature,
        encrypted,
        sighash,
        lock_time,
        value,
        script,
    })
}

pub fn letter_from_tx(
    outpoint: OutPoint,
    claim: Claim,
    tx: &Transaction,
    sighash: [u8; 32],
    adaptor_point: [u8; 33],
    adaptor_signature: [u8; 65],
    encrypted: EncryptedSecret,
) -> Result<Letter, Error> {
    let output = tx.output.first().ok_or(Error::BadPayload)?;
    Ok(Letter {
        outpoint,
        claim,
        adaptor_point,
        adaptor_signature,
        encrypted,
        sighash,
        lock_time: tx.lock_time.to_consensus_u32(),
        value: output.value,
        script: output.script_pubkey.clone(),
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(len).ok_or(Error::BadPayload)?;
        if end > self.bytes.len() {
            return Err(Error::BadPayload);
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let slice = self.bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }
}
