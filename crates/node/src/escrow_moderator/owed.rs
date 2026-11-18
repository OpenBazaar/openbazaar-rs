//! Fee payments no case keeps.
//!
//! A fee script serves a later escrow once its first case closed with
//! nothing paid to it ([`super::fee`]). A confirmed payment to a script
//! answered the assignment issued last at or before the height its
//! transaction was made at ([`FeeOutput::made`]), or the first assignment
//! when none was; so a slow payment for an old request never counts for a
//! new one. Only a payment to the request's own escrow, confirmed by the
//! due height and of at least the fee, is credited. Anything else that
//! answered a request (late, short, or after the fee was paid, on either
//! rail) is owed back to that filer's refund script.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use bitcoin::hashes::Hash;
use bitcoin::{OutPoint, Script, ScriptBuf, Txid};
use openbazaar_escrow::Party;
use openbazaar_protocol::FeePayment;
use serde_json::{json, Value};

use super::case::{fixed, party_name};
use super::fee::{EscrowFee, FeeScript};
use crate::escrow_mail::{hex, EscrowId};
use crate::wallet::FeeOutput;
use crate::Node;

/// What this node's wallet and Lightning node received for fees.
#[derive(Debug, Default)]
pub(super) struct Received {
    pub onchain: HashMap<ScriptBuf, Vec<FeeOutput>>,
    /// Sats received per invoice payment hash.
    pub lightning: HashMap<String, u64>,
}

/// A payment one filer of one escrow is owed back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Owed {
    pub filer: Party,
    pub payment: FeePayment,
    pub sats: u64,
}

impl Owed {
    pub fn value(&self) -> Value {
        json!({
            "filer": party_name(self.filer),
            "amountSats": self.sats,
            "payment": payment_id(&self.payment),
        })
    }
}

/// `txid:vout` for an on-chain payment, `ln:<payment hash>` for Lightning.
pub(super) fn payment_id(payment: &FeePayment) -> String {
    match payment {
        FeePayment::OnChain { txid, vout } => OutPoint::new(Txid::from_byte_array(*txid), *vout).to_string(),
        FeePayment::Lightning { payment_hash } => format!("ln:{}", hex(payment_hash)),
    }
}

pub(super) fn parse_payment_id(text: &str) -> Option<FeePayment> {
    if let Some(hash) = text.strip_prefix("ln:") {
        return Some(FeePayment::Lightning {
            payment_hash: fixed(hash)?,
        });
    }
    let outpoint = OutPoint::from_str(text).ok()?;
    Some(FeePayment::OnChain {
        txid: outpoint.txid.to_byte_array(),
        vout: outpoint.vout,
    })
}

fn on_chain(output: &FeeOutput) -> FeePayment {
    FeePayment::OnChain {
        txid: output.outpoint.txid.to_byte_array(),
        vout: output.outpoint.vout,
    }
}

/// The escrow and filer a payment to `script` made at height `made` answered.
pub(super) fn answered(fees: &BTreeMap<EscrowId, EscrowFee>, script: &Script, made: u32) -> Option<(EscrowId, Party)> {
    let mut assigned: Vec<(u32, EscrowId, Party)> = fees
        .iter()
        .flat_map(|(id, fee)| {
            fee.scripts
                .iter()
                .filter(|assigned| assigned.script.as_script() == script)
                .map(|assigned| (assigned.issued, *id, assigned.filer))
        })
        .collect();
    assigned.sort_by_key(|(issued, id, _)| (*issued, *id));
    let first = assigned.first().copied();
    assigned
        .into_iter()
        .rev()
        .find(|(issued, ..)| *issued <= made)
        .or(first)
        .map(|(_, id, filer)| (id, filer))
}

/// Whether `output` pays the fee: confirmed by the due height, at least the fee.
fn pays(fee: &EscrowFee, output: &FeeOutput) -> bool {
    output.height <= fee.due && output.sats >= fee.amount
}

/// The first on-chain payment escrow `id` can credit.
pub(super) fn creditable(
    fees: &BTreeMap<EscrowId, EscrowFee>,
    id: &EscrowId,
    received: &Received,
) -> Option<(FeeScript, FeeOutput)> {
    let fee = fees.get(id)?;
    fee.scripts
        .iter()
        .flat_map(|script| {
            let outputs = received.onchain.get(&script.script).into_iter().flatten();
            outputs.map(move |output| (script.clone(), *output))
        })
        .filter(|(script, output)| answered(fees, &script.script, output.made) == Some((*id, script.filer)))
        .filter(|(_, output)| pays(fee, output))
        .min_by_key(|(_, output)| (output.height, output.outpoint))
}

/// Every payment to escrow `id`'s requests that it will not keep and has
/// not sent back yet.
pub(super) fn owed(fees: &BTreeMap<EscrowId, EscrowFee>, id: &EscrowId, received: &Received) -> Vec<Owed> {
    let Some(fee) = fees.get(id) else { return Vec::new() };
    let kept = |payment: &FeePayment| {
        fee.paid.is_some_and(|paid| paid.payment == *payment)
            || fee.refunds.iter().any(|refund| refund.owed == Some(*payment))
    };
    let mut owed: Vec<Owed> = Vec::new();
    for script in &fee.scripts {
        for output in received.onchain.get(&script.script).into_iter().flatten() {
            let payment = on_chain(output);
            let pending = fee.paid.is_none() && pays(fee, output);
            if answered(fees, &script.script, output.made) == Some((*id, script.filer)) && !pending && !kept(&payment) {
                owed.push(Owed {
                    filer: script.filer,
                    payment,
                    sats: output.sats,
                });
            }
        }
    }
    for request in &fee.requests {
        let Some(hash) = request.payment_hash.as_deref() else { continue };
        let (Some(&sats), Some(payment_hash)) = (received.lightning.get(hash), fixed(hash)) else { continue };
        let payment = FeePayment::Lightning { payment_hash };
        let pending = fee.paid.is_none() && sats >= fee.amount;
        if !pending && !kept(&payment) && !owed.iter().any(|seen| seen.payment == payment) {
            owed.push(Owed {
                filer: request.filer,
                payment,
                sats,
            });
        }
    }
    owed
}

impl Node {
    /// What the wallet and the Lightning node received for `scripts` and
    /// the invoices of `hashes`. Takes the wallet lock: call it without
    /// the desk held.
    pub(super) fn fee_received(&self, scripts: &[ScriptBuf], hashes: &[String]) -> Received {
        let onchain = {
            let wallet = self.buyer_wallet();
            scripts
                .iter()
                .map(|script| (script.clone(), wallet.fee_outputs(script)))
                .filter(|(_, outputs)| !outputs.is_empty())
                .collect()
        };
        let lightning = hashes
            .iter()
            .filter_map(|hash| Some((hash.clone(), self.ln_fee_received(hash)? / 1_000)))
            .collect();
        Received { onchain, lightning }
    }

    /// [`Self::fee_received`] for every fee on the desk.
    pub(super) fn all_fees_received(&self) -> Received {
        let (scripts, hashes): (Vec<ScriptBuf>, Vec<String>) = {
            let desk = self.moderator_desk();
            let scripts = desk.fees.values().flat_map(|fee| fee.scripts.iter().map(|script| script.script.clone()));
            let hashes = desk
                .fees
                .values()
                .flat_map(|fee| fee.requests.iter().filter_map(|request| request.payment_hash.clone()));
            (scripts.collect(), hashes.collect())
        };
        self.fee_received(&scripts, &hashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escrow_moderator::fee::{Paid, Refund, Requested};

    const SCRIPT: [u8; 4] = [0x51, 0x02, 0x4e, 0x73];

    fn assigned(fee: &mut EscrowFee, filer: Party, issued: u32) {
        fee.scripts.push(FeeScript {
            filer,
            index: 3,
            script: ScriptBuf::from_bytes(SCRIPT.to_vec()),
            issued,
        });
    }

    fn output(n: u8, sats: u64, made: u32, height: u32) -> FeeOutput {
        FeeOutput {
            outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
            sats,
            height,
            made,
        }
    }

    fn received(outputs: Vec<FeeOutput>) -> Received {
        Received {
            onchain: HashMap::from([(ScriptBuf::from_bytes(SCRIPT.to_vec()), outputs)]),
            lightning: HashMap::new(),
        }
    }

    /// Escrow 1 was asked to pay the script at 100, due 200; escrow 2 got
    /// it again at 300, due 400.
    fn reused() -> BTreeMap<EscrowId, EscrowFee> {
        let mut first = EscrowFee::new([1; 32], 5_000, 200);
        assigned(&mut first, Party::Buyer, 100);
        let mut second = EscrowFee::new([2; 32], 5_000, 400);
        assigned(&mut second, Party::Vendor, 300);
        BTreeMap::from([([1; 16], first), ([2; 16], second)])
    }

    #[test]
    fn a_slow_payment_for_an_old_request_is_owed_back_not_credited_to_the_new_case() {
        let fees = reused();
        let slow = output(7, 5_000, 190, 310);
        let seen = received(vec![slow]);
        assert_eq!(answered(&fees, Script::from_bytes(&SCRIPT), slow.made), Some(([1; 16], Party::Buyer)));
        assert_eq!(creditable(&fees, &[2; 16], &seen), None, "never credited to the new case");
        assert_eq!(owed(&fees, &[2; 16], &seen), Vec::new());
        assert_eq!(
            owed(&fees, &[1; 16], &seen),
            vec![Owed { filer: Party::Buyer, payment: on_chain(&slow), sats: 5_000 }],
            "confirmed past the first case's due height: owed to its filer"
        );

        let fresh = output(8, 5_000, 305, 306);
        let seen = received(vec![slow, fresh]);
        assert_eq!(creditable(&fees, &[2; 16], &seen).map(|(_, output)| output), Some(fresh));
        let no_lock = output(9, 5_000, 320, 320);
        assert_eq!(
            creditable(&fees, &[2; 16], &received(vec![no_lock])).map(|(_, output)| output),
            Some(no_lock),
            "without a lock time, the confirmation height places it"
        );
    }

    #[test]
    fn extras_are_owed_until_sent_back_and_a_credited_payment_never_is() {
        let mut fees = reused();
        let paid = output(4, 5_000, 120, 121);
        let again = output(5, 5_000, 130, 131);
        let short = output(6, 900, 125, 126);
        let seen = received(vec![paid, short]);
        assert_eq!(creditable(&fees, &[1; 16], &seen).map(|(_, output)| output), Some(paid));
        assert_eq!(owed(&fees, &[1; 16], &seen).len(), 1, "a short payment is owed, a creditable one waits");

        let first = fees.get_mut(&[1; 16]).unwrap();
        first.paid = Some(Paid {
            filer: Party::Buyer,
            request: [0; 32],
            payment: on_chain(&paid),
            received_sats: 5_000,
            height: 121,
        });
        first.requests.push(Requested {
            filer: Party::Buyer,
            bytes: vec![1],
            digest: [0; 32],
            payment_hash: Some(hex(&[3; 32])),
            valid_until: 150,
        });
        let mut seen = received(vec![paid, again, short]);
        seen.lightning.insert(hex(&[3; 32]), 5_000);
        let ids: Vec<String> = owed(&fees, &[1; 16], &seen).iter().map(|owed| payment_id(&owed.payment)).collect();
        assert_eq!(ids, [on_chain(&again), on_chain(&short), FeePayment::Lightning { payment_hash: [3; 32] }].map(|payment| payment_id(&payment)));

        fees.get_mut(&[1; 16]).unwrap().refunds.push(Refund {
            filer: Party::Buyer,
            amount: 5_000,
            owed: Some(on_chain(&again)),
            tx: bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: Vec::new(),
                output: Vec::new(),
            },
        });
        assert_eq!(owed(&fees, &[1; 16], &seen).len(), 2, "a payment sent back is owed no more");
        for id in ["ln:0303030303030303030303030303030303030303030303030303030303030303", &payment_id(&on_chain(&again))] {
            assert_eq!(parse_payment_id(id).map(|payment| payment_id(&payment)).as_deref(), Some(id));
        }
    }
}
