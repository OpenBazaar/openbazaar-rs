//! Deprecated single-process shim for the node demo.
//!
//! The node's checkout demo still holds both commercial keys and calls
//! `fund` to show an escrow address and hold a refund. This shim keeps that
//! call compiling. It derives the funding output from the two keys and signs
//! one refund to the buyer at `timeout_height` with both keys in this
//! process. It signs no vendor claim and no dispute outcome, so the
//! moderator has no role. Remove it, and the `legacy-single-process`
//! feature, when the node moves to [`crate::BuyerSession`] /
//! [`crate::VendorSession`].

#![allow(deprecated)]

use bitcoin::absolute::{self, LOCK_TIME_THRESHOLD};
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, TxOut};
use musig2::secp::G;

use crate::crypto::{self, point_from_compressed, scalar_key};
use crate::tx::{self, funding_script};
use crate::Error;

/// Inputs to the deprecated [`fund`] shim. Holds both commercial keys.
pub struct FundParams {
    pub buyer_secret: [u8; 32],
    pub vendor_secret: [u8; 32],
    pub moderator_public: [u8; 33],
    pub funding_outpoint: OutPoint,
    pub funding_value: Amount,
    pub fee: Amount,
    pub timeout_height: u32,
    pub buyer_payout: ScriptBuf,
    pub vendor_payout: ScriptBuf,
}

/// The address and refund the node demo holds.
pub struct LegacyEscrowAddress {
    funding_outpoint: OutPoint,
    funding_output: TxOut,
    output_key: [u8; 32],
    timeout_height: u32,
    refund: Transaction,
}

impl LegacyEscrowAddress {
    pub fn funding_outpoint(&self) -> OutPoint {
        self.funding_outpoint
    }

    pub fn funding_output(&self) -> &TxOut {
        &self.funding_output
    }

    pub fn timeout_height(&self) -> u32 {
        self.timeout_height
    }

    /// Refund paying `buyer_payout` (`funding_value - fee`) at `timeout_height`.
    pub fn refund_transaction(&self) -> &Transaction {
        &self.refund
    }

    /// BIP 341 key-spend check against the funding output. Does not apply `nLockTime`.
    pub fn verify_key_spend(&self, tx: &Transaction) -> Result<(), Error> {
        if tx.input.len() != 1 || tx.input[0].previous_output != self.funding_outpoint {
            return Err(Error::BadSignature);
        }
        let signature = tx::witness_signature(tx)?;
        let sighash = tx::key_spend_sighash(tx, &self.funding_output)?;
        crypto::bip340_verify(&self.output_key, &signature, &sighash)
    }
}

#[deprecated(note = "single-process demo shim; use BuyerSession and VendorSession")]
pub fn fund(params: FundParams) -> Result<LegacyEscrowAddress, Error> {
    if params.timeout_height == 0 || params.timeout_height >= LOCK_TIME_THRESHOLD {
        return Err(Error::InvalidParams(
            "timeout height must be a non-zero block height",
        ));
    }
    if params.buyer_payout.is_empty() || params.vendor_payout.is_empty() {
        return Err(Error::InvalidParams("payout script is empty"));
    }
    let buyer_secret = scalar_key(&params.buyer_secret)?;
    let vendor_secret = scalar_key(&params.vendor_secret)?;
    let buyer = buyer_secret * G;
    let vendor = vendor_secret * G;
    let moderator = point_from_compressed(&params.moderator_public)?;
    if moderator == buyer || moderator == vendor {
        return Err(Error::InvalidParams("moderator key must be distinct"));
    }
    let refund_value = params
        .funding_value
        .checked_sub(params.fee)
        .filter(|value| *value > Amount::ZERO)
        .ok_or(Error::InvalidParams("fee exceeds the funding value"))?;
    let key_agg = crypto::aggregate(buyer, vendor)?;
    let internal = crypto::internal_xonly(buyer, vendor)?;
    let output_key = crypto::output_xonly(&key_agg);
    let funding_output = TxOut {
        value: params.funding_value,
        script_pubkey: funding_script(&internal, &output_key)?,
    };
    let lock = absolute::LockTime::from_height(params.timeout_height)
        .map_err(|_| Error::InvalidParams("timeout height"))?;
    let unsigned = tx::spend_template(
        params.funding_outpoint,
        lock,
        vec![TxOut {
            value: refund_value,
            script_pubkey: params.buyer_payout,
        }],
    );
    let sighash = tx::key_spend_sighash(&unsigned, &funding_output)?;
    let buyer_nonce = crypto::fresh_nonce(buyer_secret, &key_agg, &sighash, b"legacy/buyer");
    let vendor_nonce = crypto::fresh_nonce(vendor_secret, &key_agg, &sighash, b"legacy/vendor");
    let agg = crypto::aggregate_nonce(&buyer_nonce.public_nonce(), &vendor_nonce.public_nonce());
    let partials = [
        crypto::partial_sign(&key_agg, buyer_secret, buyer_nonce, &agg, &sighash, None)?,
        crypto::partial_sign(&key_agg, vendor_secret, vendor_nonce, &agg, &sighash, None)?,
    ];
    let signature: musig2::LiftedSignature =
        crypto::aggregate_plain(&key_agg, &agg, partials, &sighash)?;
    let address = LegacyEscrowAddress {
        funding_outpoint: params.funding_outpoint,
        funding_output,
        output_key,
        timeout_height: params.timeout_height,
        refund: tx::attach_key_spend(unsigned, &signature.serialize()),
    };
    address.verify_key_spend(&address.refund)?;
    Ok(address)
}
