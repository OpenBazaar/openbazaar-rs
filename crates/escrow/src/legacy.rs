//! Deprecated single-process shim for the node demo.
//!
//! The node's checkout demo still holds both commercial keys and calls
//! `fund` to show an escrow address. This shim keeps that call compiling.
//! It derives the funding output from the two public keys and signs nothing:
//! no refund, no vendor claim, no dispute outcome. Remove it, and the
//! `legacy-single-process` default feature, when the node moves to
//! [`crate::BuyerSession`] / [`crate::VendorSession`].

#![allow(deprecated)]

use bitcoin::absolute::LOCK_TIME_THRESHOLD;
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut};
use musig2::secp::G;

use crate::crypto::{self, point_from_compressed, scalar_key};
use crate::tx::funding_script;
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

/// The address facts the node demo displays. Nothing here can spend.
pub struct LegacyEscrowAddress {
    funding_outpoint: OutPoint,
    funding_output: TxOut,
    timeout_height: u32,
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
    let buyer = scalar_key(&params.buyer_secret)? * G;
    let vendor = scalar_key(&params.vendor_secret)? * G;
    let moderator = point_from_compressed(&params.moderator_public)?;
    if moderator == buyer || moderator == vendor {
        return Err(Error::InvalidParams("moderator key must be distinct"));
    }
    let internal = crypto::internal_xonly(buyer, vendor)?;
    let output_key = crypto::output_xonly(&crypto::aggregate(buyer, vendor)?);
    Ok(LegacyEscrowAddress {
        funding_outpoint: params.funding_outpoint,
        funding_output: TxOut {
            value: params.funding_value,
            script_pubkey: funding_script(&internal, &output_key)?,
        },
        timeout_height: params.timeout_height,
    })
}
