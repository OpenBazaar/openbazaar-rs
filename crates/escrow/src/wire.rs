//! Length-checked binary encoding for messages, terms, letters, and persisted spends.
//!
//! Integers are little-endian. Variable-length fields carry a `u32` length.

use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, Txid};
use musig2::secp::{MaybeScalar, Point};
use musig2::{AdaptorSignature, PartialSignature, PubNonce};

use crate::Error;

pub const SCRIPT_MAX: usize = 10_000;
pub const MEMO_MAX: usize = 4096;
const VAR_MAX: usize = 1 << 20;

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new(magic: &[u8; 4], version: u8) -> Self {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(magic);
        buf.push(version);
        Self { buf }
    }

    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn var(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.bytes(bytes);
    }

    pub fn amount(&mut self, amount: Amount) {
        self.u64(amount.to_sat());
    }

    pub fn script(&mut self, script: &ScriptBuf) {
        self.var(script.as_bytes());
    }

    pub fn outpoint(&mut self, outpoint: &OutPoint) {
        self.bytes(outpoint.txid.as_byte_array());
        self.u32(outpoint.vout);
    }

    pub fn point(&mut self, point: &Point) {
        self.bytes(&point.serialize());
    }

    pub fn scalar(&mut self, scalar: &MaybeScalar) {
        self.bytes(&scalar.serialize());
    }

    pub fn nonce(&mut self, nonce: &PubNonce) {
        self.bytes(&nonce.serialize());
    }

    pub fn partial(&mut self, partial: &PartialSignature) {
        self.bytes(&partial.serialize());
    }

    pub fn adaptor(&mut self, signature: &AdaptorSignature) {
        self.bytes(&signature.serialize());
    }

    pub fn tx(&mut self, tx: &Transaction) {
        self.var(&bitcoin::consensus::serialize(tx));
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], magic: &[u8; 4], version: u8) -> Result<Self, Error> {
        let mut reader = Self { bytes, at: 0 };
        if &reader.array::<4>()? != magic || reader.u8()? != version {
            return Err(Error::Malformed);
        }
        Ok(reader)
    }

    pub fn position(&self) -> usize {
        self.at
    }

    pub fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(len).ok_or(Error::Malformed)?;
        if end > self.bytes.len() {
            return Err(Error::Malformed);
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn var(&mut self, max: usize) -> Result<&'a [u8], Error> {
        let len = self.u32()? as usize;
        if len > max.min(VAR_MAX) {
            return Err(Error::Malformed);
        }
        self.take(len)
    }

    pub fn amount(&mut self) -> Result<Amount, Error> {
        Ok(Amount::from_sat(self.u64()?))
    }

    pub fn script(&mut self) -> Result<ScriptBuf, Error> {
        Ok(ScriptBuf::from_bytes(self.var(SCRIPT_MAX)?.to_vec()))
    }

    pub fn outpoint(&mut self) -> Result<OutPoint, Error> {
        let txid = Txid::from_byte_array(self.array()?);
        Ok(OutPoint {
            txid,
            vout: self.u32()?,
        })
    }

    pub fn point(&mut self) -> Result<Point, Error> {
        Point::from_slice(self.take(33)?).map_err(|_| Error::Malformed)
    }

    pub fn scalar(&mut self) -> Result<MaybeScalar, Error> {
        MaybeScalar::from_slice(self.take(32)?).map_err(|_| Error::Malformed)
    }

    pub fn nonce(&mut self) -> Result<PubNonce, Error> {
        PubNonce::from_bytes(self.take(66)?).map_err(|_| Error::Malformed)
    }

    pub fn partial(&mut self) -> Result<PartialSignature, Error> {
        PartialSignature::from_slice(self.take(32)?).map_err(|_| Error::Malformed)
    }

    pub fn adaptor(&mut self) -> Result<AdaptorSignature, Error> {
        AdaptorSignature::from_bytes(self.take(65)?).map_err(|_| Error::Malformed)
    }

    pub fn tx(&mut self) -> Result<Transaction, Error> {
        bitcoin::consensus::deserialize(self.var(VAR_MAX)?).map_err(|_| Error::Malformed)
    }

    pub fn finish(self) -> Result<(), Error> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::Malformed)
        }
    }
}
