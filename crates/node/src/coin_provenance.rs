//! Where a wallet's own change came from.
//!
//! A payment's change output can be spent in the next payment. If that next
//! payment pays someone else, the two recipients are linked on chain. Change
//! stays linked to the recipient it was created for, including change of
//! that change, and a fee bump that pays no one inherits the same link.
//! Coins received from outside the wallet are fresh: this policy does not
//! pretend that separate outputs of one incoming payment are unlinkable.
//!
//! The classification is read from the wallet's transactions, so it is still
//! there after a restart. A cycle or a history deeper than [`MAX_DEPTH`] is
//! mixed, and mixed coins are not spent unless the caller opts in.

use std::collections::HashSet;

use bdk_wallet::{KeychainKind, Wallet};
use bitcoin::{OutPoint, Script, ScriptBuf};

pub(crate) const LINK_EXPLANATION: &str = "Spending change from a payment to someone else would link the two on chain. Separate transactions, or separate outputs from the same funding, do not make those purchases unlinkable. Send \"linkCoins\": true only if you choose to link them.";

const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Provenance {
    /// An external receive, or change that has only spent those.
    Fresh,
    /// Change created while paying `script`, or change of that change.
    Linked(ScriptBuf),
    /// Inputs that point at more than one recipient, or a history that is
    /// too deep or cyclic to trust.
    Mixed,
}

/// Unspent change this payment must not spend. `allowed` is the one
/// recipient whose linked change may be used. `None` allows only fresh
/// coins. `link_coins` is the caller's explicit decision to ignore this.
pub(crate) fn blocked_coins(
    wallet: &Wallet,
    allowed: Option<&Script>,
    link_coins: bool,
) -> Vec<OutPoint> {
    if link_coins {
        return Vec::new();
    }
    wallet
        .list_unspent()
        .filter(|utxo| utxo.keychain == KeychainKind::Internal)
        .filter(|utxo| !compatible(&provenance(wallet, utxo.outpoint), allowed))
        .map(|utxo| utxo.outpoint)
        .collect()
}

pub(crate) fn provenance(wallet: &Wallet, outpoint: OutPoint) -> Provenance {
    classify(wallet, outpoint, &mut HashSet::new(), 0)
}

fn compatible(provenance: &Provenance, allowed: Option<&Script>) -> bool {
    match provenance {
        Provenance::Fresh => true,
        Provenance::Linked(script) => allowed.is_some_and(|allowed| allowed == script.as_script()),
        Provenance::Mixed => false,
    }
}

fn classify(
    wallet: &Wallet,
    outpoint: OutPoint,
    seen: &mut HashSet<OutPoint>,
    depth: usize,
) -> Provenance {
    if depth > MAX_DEPTH || !seen.insert(outpoint) {
        return Provenance::Mixed;
    }
    let Some(canonical) = wallet.get_tx(outpoint.txid) else {
        return Provenance::Fresh;
    };
    let tx = canonical.tx_node.tx.clone();
    let Some(output) = tx.output.get(outpoint.vout as usize) else {
        return Provenance::Mixed;
    };
    if keychain_of(wallet, &output.script_pubkey) != Some(KeychainKind::Internal) {
        return Provenance::Fresh;
    }
    let mut foreign: Option<ScriptBuf> = None;
    let mut inherited = Provenance::Fresh;
    for input in &tx.input {
        let Some(prev) = wallet.get_tx(input.previous_output.txid) else {
            continue;
        };
        let prev_tx = prev.tx_node.tx.clone();
        let Some(prev_out) = prev_tx.output.get(input.previous_output.vout as usize) else {
            return Provenance::Mixed;
        };
        match keychain_of(wallet, &prev_out.script_pubkey) {
            Some(KeychainKind::Internal) => {
                inherited = merge(
                    inherited,
                    classify(wallet, input.previous_output, seen, depth + 1),
                );
            }
            Some(KeychainKind::External) => {}
            None if wallet.is_mine(prev_out.script_pubkey.clone()) => {
                return Provenance::Mixed;
            }
            None => {}
        }
    }
    for out in &tx.output {
        if wallet.is_mine(out.script_pubkey.clone()) {
            continue;
        }
        foreign = match foreign {
            None => Some(out.script_pubkey.clone()),
            Some(existing) if existing == out.script_pubkey => Some(existing),
            Some(_) => return Provenance::Mixed,
        };
    }
    match (foreign, inherited) {
        (None, inherited) => inherited,
        (Some(script), Provenance::Fresh) => Provenance::Linked(script),
        (Some(script), Provenance::Linked(existing)) if existing == script => {
            Provenance::Linked(script)
        }
        (Some(_), _) => Provenance::Mixed,
    }
}

fn merge(left: Provenance, right: Provenance) -> Provenance {
    match (left, right) {
        (Provenance::Mixed, _) | (_, Provenance::Mixed) => Provenance::Mixed,
        (Provenance::Fresh, other) | (other, Provenance::Fresh) => other,
        (Provenance::Linked(left), Provenance::Linked(right)) if left == right => {
            Provenance::Linked(left)
        }
        (Provenance::Linked(_), Provenance::Linked(_)) => Provenance::Mixed,
    }
}

fn keychain_of(wallet: &Wallet, script: &Script) -> Option<KeychainKind> {
    wallet
        .derivation_of_spk(script.to_owned())
        .map(|(keychain, _)| keychain)
}
