//! Acceptance policy and output standardness.
//!
//! A [`Policy`] is applied twice at funding: once as the party's own floor,
//! and once as the moderator's published policy carried in the signed terms.
//! The moderator later checks letters against the policy in the terms, so
//! terms that pass funding cannot be refused by a stricter private policy.

use bitcoin::{Amount, Script};

use crate::wire::{Reader, Writer};
use crate::Error;

/// Value of the pay-to-anchor output on every presigned spend.
pub const ANCHOR_VALUE: Amount = Amount::from_sat(240);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Minimum fee rate for every presigned transaction, in sat/vB, measured
    /// on the actual signed vsize. The anchor lets the recipient CPFP above it.
    pub min_fee_rate_sat_vb: u64,
    /// Largest `network_fee` accepted for one presigned transaction.
    pub max_network_fee: Amount,
    /// Minimum blocks from the funding tip to `dispute_deadline`, so the
    /// buyer has time to receive the goods before the window closes.
    pub min_delivery_window: u32,
    /// Minimum blocks between `dispute_deadline` and `vendor_claim_height`.
    /// The moderator must rule, and the winner must confirm, inside this gap.
    pub min_ruling_window: u32,
    /// Minimum blocks between `vendor_claim_height` and `refund_height`.
    pub min_claim_window: u32,
    /// Largest `refund_height - tip` accepted at funding.
    pub max_refund_horizon: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min_fee_rate_sat_vb: 4,
            max_network_fee: Amount::from_sat(50_000),
            min_delivery_window: 1_008,
            min_ruling_window: 144,
            min_claim_window: 144,
            max_refund_horizon: 12_960,
        }
    }
}

impl Policy {
    pub(crate) fn write(&self, w: &mut Writer) {
        w.u64(self.min_fee_rate_sat_vb);
        w.amount(self.max_network_fee);
        w.u32(self.min_delivery_window);
        w.u32(self.min_ruling_window);
        w.u32(self.min_claim_window);
        w.u32(self.max_refund_horizon);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            min_fee_rate_sat_vb: r.u64()?,
            max_network_fee: r.amount()?,
            min_delivery_window: r.u32()?,
            min_ruling_window: r.u32()?,
            min_claim_window: r.u32()?,
            max_refund_horizon: r.u32()?,
        })
    }
}

/// Bitcoin Core's dust threshold for a standard payout script, or `None` if
/// the script is not P2WPKH, P2WSH, P2TR, P2PKH, or P2SH.
pub fn payout_dust_limit(script: &Script) -> Option<Amount> {
    let standard = script.is_p2wpkh()
        || script.is_p2wsh()
        || script.is_p2tr()
        || script.is_p2pkh()
        || script.is_p2sh();
    standard.then(|| script.minimal_non_dust())
}

pub(crate) fn check_payout(script: &Script, value: Amount) -> Result<(), Error> {
    let dust = payout_dust_limit(script).ok_or(Error::NonStandardOutput)?;
    if value < dust {
        return Err(Error::NonStandardOutput);
    }
    Ok(())
}
