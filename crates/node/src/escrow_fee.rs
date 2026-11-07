//! Escrow fees: the presigned fee, and CPFP through the P2A anchor.
//!
//! Every presigned spend pays the same `network_fee`, fixed in the terms
//! before funding and usually spent days later. The buyer picks it at
//! checkout from its [`crate::ChainSource`] estimate (compact block filters
//! average recent blocks), times the largest spend's vsize, clamped between
//! the moderator policy's floor (`min_fee_rate_sat_vb` on that vsize) and
//! cap (`max_network_fee`). The vendor accepts any fee the policy accepts:
//! the fee comes out of the buyer's funding, and a fee too low by the time
//! a spend is broadcast is fixed with a child.
//!
//! Every presigned spend except the release carries a 240-sat P2A anchor.
//! [`cpfp_child`] spends it with one of the node wallet's coins and sends
//! the rest back to the wallet, paying enough that parent and child
//! together reach the target rate.

use bdk_wallet::KeychainKind;
use bitcoin::psbt::Input as PsbtInput;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, Weight, Witness};
use openbazaar_escrow::{anchor_script, max_spend_vsize, EscrowTerms, ModeratorRecord};

use crate::seed::Seed;
use crate::wallet::NodeWallet;

/// Witness weight of a P2A spend: an empty witness stack and script.
const P2A_SATISFACTION: Weight = Weight::from_wu(4);

/// The largest signed vsize any spend can have under `moderator`: P2TR
/// payouts are the longest standard payout scripts.
pub(crate) fn vsize_bound(moderator: &ModeratorRecord) -> u64 {
    let p2tr = ScriptBuf::from_bytes([&[0x51, 0x20][..], &[0x11; 32]].concat());
    let terms = EscrowTerms {
        buyer: [2; 33],
        vendor: [3; 33],
        moderator: moderator.clone(),
        funding_value: Amount::from_sat(100_000_000),
        network_fee: moderator.policy.max_network_fee,
        dispute_deadline: 1_000,
        vendor_claim_height: 2_000,
        refund_height: 3_000,
        buyer_payout: p2tr.clone(),
        vendor_payout: p2tr,
    };
    max_spend_vsize(&terms).unwrap_or(250)
}

/// The presigned fee for an estimate of `rate` sat/vB (`None`: no estimate).
pub(crate) fn presigned_fee(rate: Option<f64>, moderator: &ModeratorRecord) -> u64 {
    let policy = moderator.policy;
    let vsize = vsize_bound(moderator);
    let floor = policy.min_fee_rate_sat_vb.max(1).saturating_mul(vsize);
    let cap = policy.max_network_fee.to_sat().max(floor);
    let wanted = rate
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map(|rate| (rate * vsize as f64).ceil() as u64)
        .unwrap_or(floor);
    wanted.clamp(floor, cap)
}

/// A signed child of an escrow spend.
#[derive(Debug)]
pub(crate) struct Child {
    pub tx: Transaction,
    pub fee_sats: u64,
    /// Fee rate of parent and child together, sat/vB.
    pub package_rate: f64,
}

/// The anchor output of `parent`.
pub(crate) fn anchor_vout(parent: &Transaction) -> Option<u32> {
    let anchor = anchor_script();
    parent.output.iter().position(|out| out.script_pubkey == anchor).map(|vout| vout as u32)
}

/// Build and sign a child spending `parent`'s anchor with this wallet's
/// coins, never touching `unspendable`. Parent and child together pay
/// `target_rate` sat/vB; `parent_fee` is what the parent pays alone.
pub(crate) fn cpfp_child(
    wallet: &mut NodeWallet,
    seed: &Seed,
    parent: &Transaction,
    parent_fee: u64,
    target_rate: f64,
    unspendable: &[OutPoint],
) -> Result<Child, String> {
    if !target_rate.is_finite() || target_rate < 1.0 {
        return Err("a fee rate of at least 1 sat/vB is needed".into());
    }
    let vout = anchor_vout(parent).ok_or("this transaction has no anchor to bump")?;
    let parent_vsize = parent.vsize() as u64;
    // Sign once to learn the child's size, then again with the exact fee.
    let probe = sign_child(wallet, seed, parent, vout, 1_000, unspendable)?;
    let child_vsize = probe.vsize() as u64;
    let package = (target_rate * (parent_vsize + child_vsize) as f64).ceil() as u64;
    let fee = package.saturating_sub(parent_fee).max(child_vsize);
    let tx = sign_child(wallet, seed, parent, vout, fee, unspendable)?;
    let package_rate = (parent_fee + fee) as f64 / (parent_vsize + tx.vsize() as u64) as f64;
    Ok(Child {
        tx,
        fee_sats: fee,
        package_rate,
    })
}

fn sign_child(
    wallet: &mut NodeWallet,
    seed: &Seed,
    parent: &Transaction,
    vout: u32,
    fee: u64,
    unspendable: &[OutPoint],
) -> Result<Transaction, String> {
    let anchor = OutPoint {
        txid: parent.compute_txid(),
        vout,
    };
    let input = PsbtInput {
        witness_utxo: Some(parent.output[vout as usize].clone()),
        non_witness_utxo: Some(parent.clone()),
        ..Default::default()
    };
    let change = wallet.wallet.next_unused_address(KeychainKind::Internal).script_pubkey();
    let mut psbt = {
        let mut builder = wallet.wallet.build_tx();
        builder
            .add_foreign_utxo(anchor, input, P2A_SATISFACTION)
            .map_err(|err| err.to_string())?
            .drain_to(change)
            .fee_absolute(Amount::from_sat(fee));
        for outpoint in unspendable {
            builder.add_unspendable(*outpoint);
        }
        builder
            .finish()
            .map_err(|err| format!("the wallet cannot pay a {fee}-sat bump: {err}"))?
    };
    let index = psbt
        .unsigned_tx
        .input
        .iter()
        .position(|txin| txin.previous_output == anchor)
        .ok_or("the bump lost its anchor input")?;
    psbt.inputs[index].final_script_witness = Some(Witness::new());
    psbt.inputs[index].final_script_sig = Some(ScriptBuf::new());
    // The anchor is not taproot, so the signer wants every input's full previous transaction.
    for (input, txin) in psbt.inputs.iter_mut().zip(&psbt.unsigned_tx.input) {
        if input.non_witness_utxo.is_none() {
            let previous = wallet.wallet.get_tx(txin.previous_output.txid).ok_or("the wallet lost a coin's transaction")?;
            input.non_witness_utxo = Some((*previous.tx_node.tx).clone());
        }
    }
    if !wallet.sign(&mut psbt, seed).map_err(|err| err.to_string())? {
        return Err("the bump could not be fully signed".into());
    }
    wallet.persist().map_err(|err| err.to_string())?;
    psbt.extract_tx().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow_fixture::moderator_record;
    use crate::network::NetworkMode;

    #[test]
    fn the_presigned_fee_follows_the_estimate_within_the_policy_floor_and_cap() {
        let record = moderator_record(NetworkMode::Testnet4).unwrap();
        let vsize = vsize_bound(&record);
        assert!((150..250).contains(&vsize), "{vsize}");
        let floor = record.policy.min_fee_rate_sat_vb * vsize;
        assert_eq!(presigned_fee(None, &record), floor);
        assert_eq!(presigned_fee(Some(1.0), &record), floor, "never below the floor");
        assert_eq!(presigned_fee(Some(f64::NAN), &record), floor);
        assert_eq!(presigned_fee(Some(10.0), &record), 10 * vsize);
        assert_eq!(
            presigned_fee(Some(1e9), &record),
            record.policy.max_network_fee.to_sat(),
            "never above the cap"
        );
    }
}
