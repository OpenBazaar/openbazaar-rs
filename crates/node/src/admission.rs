//! The directory fee.
//!
//! A store pays `fee_sats` to its own fee address: the fee key tweaked by the
//! store id ([`store_fee_key`]). The head schedules those terms; a payment is
//! judged by the entry in force at the height that confirmed it, so a new
//! entry reaches each store when it next pays. The seller's node pays from its
//! wallet, or the seller pays from any wallet, exchange, or swap. After
//! `fee_confs` confirmations inside `epoch_blocks` of the tip, the store key
//! signs an admission over that outpoint, and over a new payment once the old
//! one leaves the epoch. Version 1 admissions paid the head's fee key with an
//! OP_RETURN of `OBAD` and the store id; buyers still accept them. The fee is
//! a cost. It is not the reputation bond and it is not a sale: the
//! transaction has no order and no buyer key.

use std::collections::BTreeSet;

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::script::PushBytesBuf;
use bitcoin::secp256k1::{Secp256k1, XOnlyPublicKey};
use bitcoin::{Address, Amount, Network, Script, ScriptBuf, Transaction, Txid};
use serde_json::{json, Value};

use openbazaar_protocol::{store_fee_key, Admission, DirectoryTerms, SeedDocument, TermsSchedule};

use crate::chain::ChainSource;
use crate::http::{json_body, local_guard, Client};
use crate::network::NetworkMode;
use crate::store_profile::Role;
use crate::Node;

/// The head's fee key, which version 1 admissions pay.
pub(crate) fn fee_script(fee_key: &[u8; 32], network: Network) -> Result<ScriptBuf, String> {
    let key = XOnlyPublicKey::from_slice(fee_key).map_err(|_| "the head's fee key is not a public key".to_string())?;
    Ok(Address::p2tr(&Secp256k1::new(), key, None, network).script_pubkey())
}

/// The address a version 2 fee for `store_id` pays.
pub(crate) fn store_fee_address(fee_key: &[u8; 32], store_id: &[u8; 32], network: Network) -> Result<Address, String> {
    let key = store_fee_key(fee_key, store_id).map_err(|_| "the head's fee key is not a public key".to_string())?;
    let key = XOnlyPublicKey::from_slice(&key).map_err(|_| "the store fee key is not a public key".to_string())?;
    Ok(Address::p2tr(&Secp256k1::new(), key, None, network))
}

pub(crate) fn fee_return(store_id: &[u8; 32]) -> Result<ScriptBuf, String> {
    let mut payload = Vec::with_capacity(36);
    payload.extend_from_slice(b"OBAD");
    payload.extend_from_slice(store_id);
    let push = PushBytesBuf::try_from(payload).map_err(|_| "the admission label does not fit an output".to_string())?;
    Ok(ScriptBuf::new_op_return(&push))
}

/// The payment proves the store id, a real output of at least `fee_sats` on
/// the address the admission's version names, `fee_confs` confirmations, and
/// a height inside `epoch_blocks` of the tip, all from the head's entry in
/// force at that height. It does not prove honesty, direct pay, or the bond.
pub(crate) fn fee_confirmed(
    chain: &dyn ChainSource,
    network: NetworkMode,
    head: &SeedDocument,
    admission: &Admission,
) -> bool {
    if admission.verify().is_err() {
        return false;
    }
    let Ok(tip) = chain.tip_height() else { return false };
    let Some(entry) = head.schedule.at(admission.height) else { return false };
    let terms = &entry.terms;
    let script = match admission.version {
        1 => fee_script(&terms.fee_key, network.bitcoin()),
        _ => store_fee_address(&terms.fee_key, &admission.store_id, network.bitcoin()).map(|address| address.script_pubkey()),
    };
    let Ok(script) = script else { return false };
    let from = tip.saturating_sub(terms.epoch_blocks);
    if chain.script_txs(script.as_script(), from).is_err() {
        return false;
    }
    let txid = Txid::from_byte_array(admission.txid);
    let Ok(Some((tx, height))) = chain.transaction(&txid) else { return false };
    let Some(height) = height else { return false };
    if height != admission.height {
        return false;
    }
    let age = tip.saturating_sub(height);
    if age > terms.epoch_blocks {
        return false;
    }
    let confs = age.saturating_add(1);
    if confs < terms.fee_confs {
        return false;
    }
    let Some(output) = tx.output.get(admission.vout as usize) else { return false };
    if output.script_pubkey != script || output.value.to_sat() < terms.fee_sats {
        return false;
    }
    if admission.version != 1 {
        return true;
    }
    let Ok(label) = fee_return(&admission.store_id) else { return false };
    tx.output.iter().any(|output| output.script_pubkey == label)
}

/// Store ids among `admissions` whose fee payment `chain` confirms now.
pub(crate) fn live_admissions(
    chain: &dyn ChainSource,
    network: NetworkMode,
    head: Option<&SeedDocument>,
    admissions: &[Admission],
) -> BTreeSet<[u8; 32]> {
    let Some(head) = head else { return BTreeSet::new() };
    admissions.iter().filter(|admission| fee_confirmed(chain, network, head, admission)).map(|admission| admission.store_id).collect()
}

enum FeePayment {
    Paid { txid: Txid, vout: u32, height: u32 },
    Pending,
    Unpaid,
}

/// Blocks before a new entry starts in which a fee may still confirm under either entry.
pub(crate) const GUARD_BLOCKS: u32 = 6;

/// The terms a fee broadcast at `tip` pays. It confirms at `tip + 1` at the
/// earliest. When the next entry starts inside [`GUARD_BLOCKS`] of that and
/// names the same fee key, the fee is the higher amount, so it passes on either
/// side of the boundary. A new fee key inside the guard is paid as the current
/// entry: `sign-seed` does not sign one yet.
pub(crate) fn payment_terms(schedule: &TermsSchedule, tip: u32) -> Option<DirectoryTerms> {
    let next_block = tip.saturating_add(1);
    let mut pay = schedule.at(next_block)?.terms;
    if let Some(next) = schedule.after(next_block) {
        if next.start_height <= next_block.saturating_add(GUARD_BLOCKS) && next.terms.fee_key == pay.fee_key {
            pay.fee_sats = pay.fee_sats.max(next.terms.fee_sats);
        }
    }
    Some(pay)
}

/// Last height an admission confirmed at `height` is listed.
pub(crate) fn listed_until(schedule: &TermsSchedule, height: u32) -> u32 {
    height.saturating_add(schedule.at(height).map_or(0, |entry| entry.terms.epoch_blocks))
}

/// The newest output on `script` that pays the entry in force at its height,
/// inside that entry's epoch with enough confirmations, else whether one is
/// still confirming. Only entries with `fee_key` name `script`. A mempool
/// output counts as confirming when it pays the entry in force at the next block.
fn fee_payment(chain: &dyn ChainSource, schedule: &TermsSchedule, fee_key: &[u8; 32], script: &Script) -> Result<FeePayment, String> {
    let tip = chain.tip_height()?;
    let least = schedule.at(tip.saturating_add(1)).map_or(u64::MAX, |entry| entry.terms.fee_sats);
    let mut paid: Option<(Txid, u32, u32)> = None;
    let mut pending = false;
    for tx in chain.script_txs(script, tip.saturating_sub(schedule.max_epoch()))? {
        let Some(height) = tx.height else {
            pending |= tx.outputs.iter().any(|(_, sats)| *sats >= least);
            continue;
        };
        let Some(terms) = schedule.at(height).map(|entry| entry.terms) else { continue };
        if terms.fee_key != *fee_key {
            continue;
        }
        let Some(&(vout, _)) = tx.outputs.iter().find(|(_, sats)| *sats >= terms.fee_sats) else { continue };
        let age = tip.saturating_sub(height);
        if age > terms.epoch_blocks {
            continue;
        }
        if age.saturating_add(1) < terms.fee_confs {
            pending = true;
        } else if paid.is_none_or(|(_, _, best)| height > best) {
            paid = Some((tx.txid, vout, height));
        }
    }
    Ok(match (paid, pending) {
        (Some((txid, vout, height)), _) => FeePayment::Paid { txid, vout, height },
        (None, true) => FeePayment::Pending,
        (None, false) => FeePayment::Unpaid,
    })
}

/// This store's fee as the last directory pass saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FeeState {
    Listed { until: u32 },
    Confirming,
    Unpaid { error: Option<String> },
}

/// The fee state and the chain tip it was seen at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeeSeen {
    state: FeeState,
    tip: u32,
}

impl Node {
    /// Admit the own store for this epoch. A payment to the store's fee
    /// address from anywhere counts. With none, the node pays from its wallet
    /// unless its last fee has not confirmed yet. A missing head, mainnet, or
    /// a node that has not chosen to sell does nothing: a buyer's wallet
    /// never pays this. No new head sequence is signed.
    pub(crate) fn publish_admission(&self) -> Result<(), String> {
        if self.network != NetworkMode::Testnet4 || self.role() != Some(Role::Seller) {
            return Ok(());
        }
        let Some(head) = self.seed_head() else { return Ok(()) };
        let store = self.own_record().ok_or_else(|| "the own store has no signed record yet".to_string())?;
        let id = *store.id.as_bytes();
        let tip = self.chain.tip_height()?;
        if let Some(bytes) = self.read_outbox("admission") {
            if let Ok(admission) = Admission::decode(&bytes) {
                if admission.store_id == id && fee_confirmed(self.chain.as_ref(), self.network, &head, &admission) {
                    self.set_fee_state(FeeState::Listed { until: listed_until(&head.schedule, admission.height) }, tip);
                    return Ok(());
                }
            }
        }
        let Some(pay) = payment_terms(&head.schedule, tip) else { return Ok(()) };
        let script = store_fee_address(&pay.fee_key, &id, self.network.bitcoin())?.script_pubkey();
        let (txid, vout, height) = match fee_payment(self.chain.as_ref(), &head.schedule, &pay.fee_key, &script)? {
            FeePayment::Paid { txid, vout, height } => (txid, vout, height),
            FeePayment::Pending => {
                self.set_fee_state(FeeState::Confirming, tip);
                return Ok(());
            }
            FeePayment::Unpaid if !self.last_fee_confirmed()? => {
                self.set_fee_state(FeeState::Confirming, tip);
                return Ok(());
            }
            FeePayment::Unpaid => {
                if let Err(err) = self.broadcast_fee(&script, pay.fee_sats) {
                    self.set_fee_state(FeeState::Unpaid { error: Some(err.clone()) }, tip);
                    return Err(err);
                }
                self.set_fee_state(FeeState::Confirming, tip);
                return Ok(());
            }
        };
        let signer = self.own_signing_key().ok_or_else(|| "unlock the wallet to sign the admission".to_string())?;
        let admission = Admission::sign(id, txid.to_byte_array(), vout, height, &signer).map_err(|err| err.to_string())?;
        if !fee_confirmed(self.chain.as_ref(), self.network, &head, &admission) {
            return Ok(());
        }
        {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.queue_admission(&admission.encode().map_err(|err| err.to_string())?)?;
            replica.queue_store(&store)?;
        }
        self.set_fee_state(FeeState::Listed { until: listed_until(&head.schedule, height) }, tip);
        Ok(())
    }

    /// Whether the fee this node broadcast last has confirmed, or it never
    /// broadcast one. A server that has not seen that transaction yet must
    /// not make the node pay twice.
    fn last_fee_confirmed(&self) -> Result<bool, String> {
        let Some(tx) = self.read_outbox("fee").and_then(|bytes| deserialize::<Transaction>(&bytes).ok()) else {
            return Ok(true);
        };
        Ok(matches!(self.chain.transaction(&tx.compute_txid())?, Some((_, Some(_)))))
    }

    fn broadcast_fee(&self, script: &Script, sats: u64) -> Result<Transaction, String> {
        self.sync_buyer_wallet()?;
        let fee_rate = self.pay_fee_rate().map_err(|err| format!("{err:?}"))?;
        let mut psbt = {
            let mut wallet = self.buyer_wallet();
            let untrusted = wallet.untrusted_pending();
            let mut builder = wallet.wallet.build_tx();
            builder.add_recipient(script.to_owned(), Amount::from_sat(sats)).fee_rate(fee_rate);
            for outpoint in untrusted.into_iter().chain(self.escrow.coins.outpoints()) {
                builder.add_unspendable(outpoint);
            }
            let psbt = builder.finish().map_err(|err| format!("the wallet cannot pay the directory fee: {err}"))?;
            wallet.persist().map_err(|err| err.to_string())?;
            psbt
        };
        let tx = {
            let wallet = self.buyer_wallet();
            let seed = self.spend_seed(&wallet).map_err(|err| format!("{err:?}"))?;
            if !wallet.sign(&mut psbt, &seed).map_err(|err| err.to_string())? {
                return Err("the directory fee could not be fully signed".into());
            }
            psbt.extract_tx().map_err(|err| err.to_string())?
        };
        self.chain.broadcast(&tx)?;
        {
            let mut wallet = self.buyer_wallet();
            wallet.wallet.apply_unconfirmed_txs([(tx.clone(), crate::buyer_orders::now_secs())]);
            let _ = wallet.persist();
        }
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        replica.write_outbox("fee", &serialize(&tx))?;
        Ok(tx)
    }

    fn seed_head(&self) -> Option<SeedDocument> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).head()
    }

    fn set_fee_state(&self, state: FeeState, tip: u32) {
        *self.directory_fee.lock().unwrap_or_else(|err| err.into_inner()) = Some(FeeSeen { state, tip });
    }

    fn read_outbox(&self, name: &str) -> Option<Vec<u8>> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).read_outbox(name)
    }

    /// The directory-fee transaction this node already broadcast, if it has one.
    pub(crate) fn directory_fee_txid(&self) -> Option<String> {
        let bytes = self.read_outbox("fee")?;
        let tx: Transaction = deserialize(&bytes).ok()?;
        Some(tx.compute_txid().to_string())
    }

    /// The own store's fee address, amount, and state for `GET /directory/fee`.
    pub(crate) fn directory_fee_value(&self) -> Value {
        let unavailable = |reason: &str| json!({ "available": false, "reason": reason });
        if self.network != NetworkMode::Testnet4 {
            return unavailable("The directory fee runs on testnet4 only.");
        }
        if !self.local.introductions {
            return unavailable("Introductions are off on this node, so it is not listed in the directory.");
        }
        let Some(head) = self.seed_head() else {
            return unavailable("This node has not fetched the directory yet.");
        };
        let Some(store) = self.own_record() else {
            return unavailable("Open your store first.");
        };
        let seen = self.directory_fee.lock().unwrap_or_else(|err| err.into_inner()).clone();
        let first = head.schedule.entries()[0];
        let (pay, after) = match &seen {
            Some(seen) => (payment_terms(&head.schedule, seen.tip), seen.tip.saturating_add(1)),
            None => (Some(first.terms), first.start_height),
        };
        let Some(pay) = pay else {
            return unavailable("The directory's fee terms have not started yet.");
        };
        let fee_address = |terms: &DirectoryTerms| store_fee_address(&terms.fee_key, store.id.as_bytes(), self.network.bitcoin()).ok();
        let Some(address) = fee_address(&pay) else {
            return unavailable("The directory's fee key is not a public key.");
        };
        let upcoming: Vec<Value> = head
            .schedule
            .entries()
            .iter()
            .filter(|entry| entry.start_height > after)
            .filter_map(|entry| {
                let next = fee_address(&entry.terms)?;
                Some(json!({
                    "startHeight": entry.start_height,
                    "announcedHeight": entry.announced_height,
                    "address": next.to_string(),
                    "addressChanges": next != address,
                    "sats": entry.terms.fee_sats,
                    "confirmations": entry.terms.fee_confs,
                    "epochBlocks": entry.terms.epoch_blocks,
                }))
            })
            .collect();
        let refused = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).head_refused();
        let tip = seen.as_ref().map(|seen| seen.tip);
        let (state, until, error) = match seen.map(|seen| seen.state) {
            Some(FeeState::Listed { until }) => ("listed", Some(until), None),
            Some(FeeState::Confirming) => ("confirming", None, None),
            Some(FeeState::Unpaid { error }) => ("unpaid", None, error),
            None => ("checking", None, None),
        };
        json!({
            "available": true,
            "address": address.to_string(),
            "sats": pay.fee_sats,
            "confirmations": pay.fee_confs,
            "epochBlocks": pay.epoch_blocks,
            "state": state,
            "listedUntilHeight": until,
            "error": error,
            "tipHeight": tip,
            "upcoming": upcoming,
            "headRefused": refused.map(|(sequence, reason)| json!({ "sequence": sequence, "reason": reason })),
        })
    }
}

/// `GET /directory/fee`: where to pay this store's directory fee, and whether it is listed.
pub(crate) fn fee_response(node: &Node, client: &Client, head: &str, origin: Option<&str>) -> Vec<u8> {
    if let Err(response) = local_guard(node, client, head, origin) {
        return response;
    }
    json_body("200 OK", &node.directory_fee_value(), origin)
}
