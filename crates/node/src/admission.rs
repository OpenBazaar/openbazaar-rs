//! The directory fee.
//!
//! A store pays `fee_sats` to its own fee address: the head's fee key tweaked
//! by the store id ([`store_fee_key`]). The seller's node pays it from its
//! wallet, or the seller pays it from any wallet, exchange, or swap. After
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

use openbazaar_protocol::{store_fee_key, Admission, SeedDocument};

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
/// a height inside `epoch_blocks` of the tip. It does not prove honesty,
/// direct pay, or the bond.
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
    let script = match admission.version {
        1 => fee_script(&head.terms.fee_key, network.bitcoin()),
        _ => store_fee_address(&head.terms.fee_key, &admission.store_id, network.bitcoin()).map(|address| address.script_pubkey()),
    };
    let Ok(script) = script else { return false };
    let from = tip.saturating_sub(head.terms.epoch_blocks);
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
    if age > head.terms.epoch_blocks {
        return false;
    }
    let confs = age.saturating_add(1);
    if confs < head.terms.fee_confs {
        return false;
    }
    let Some(output) = tx.output.get(admission.vout as usize) else { return false };
    if output.script_pubkey != script || output.value.to_sat() < head.terms.fee_sats {
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

/// The newest output on `script` of at least the fee that is inside the
/// epoch with enough confirmations, else whether one is still confirming.
fn fee_payment(chain: &dyn ChainSource, head: &SeedDocument, script: &Script) -> Result<FeePayment, String> {
    let tip = chain.tip_height()?;
    let terms = &head.terms;
    let mut paid: Option<(Txid, u32, u32)> = None;
    let mut pending = false;
    for tx in chain.script_txs(script, tip.saturating_sub(terms.epoch_blocks))? {
        let Some(&(vout, _)) = tx.outputs.iter().find(|(_, sats)| *sats >= terms.fee_sats) else { continue };
        match tx.height {
            None => pending = true,
            Some(height) if tip.saturating_sub(height) > terms.epoch_blocks => {}
            Some(height) if tip.saturating_sub(height).saturating_add(1) < terms.fee_confs => pending = true,
            Some(height) if paid.is_none_or(|(_, _, best)| height > best) => paid = Some((tx.txid, vout, height)),
            Some(_) => {}
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
        if let Some(bytes) = self.read_outbox("admission") {
            if let Ok(admission) = Admission::decode(&bytes) {
                if admission.store_id == id && fee_confirmed(self.chain.as_ref(), self.network, &head, &admission) {
                    self.set_fee_state(FeeState::Listed { until: admission.height.saturating_add(head.terms.epoch_blocks) });
                    return Ok(());
                }
            }
        }
        let script = store_fee_address(&head.terms.fee_key, &id, self.network.bitcoin())?.script_pubkey();
        let (txid, vout, height) = match fee_payment(self.chain.as_ref(), &head, &script)? {
            FeePayment::Paid { txid, vout, height } => (txid, vout, height),
            FeePayment::Pending => {
                self.set_fee_state(FeeState::Confirming);
                return Ok(());
            }
            FeePayment::Unpaid if !self.last_fee_confirmed()? => {
                self.set_fee_state(FeeState::Confirming);
                return Ok(());
            }
            FeePayment::Unpaid => {
                if let Err(err) = self.broadcast_fee(&head, &script) {
                    self.set_fee_state(FeeState::Unpaid { error: Some(err.clone()) });
                    return Err(err);
                }
                self.set_fee_state(FeeState::Confirming);
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
        self.set_fee_state(FeeState::Listed { until: height.saturating_add(head.terms.epoch_blocks) });
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

    fn broadcast_fee(&self, head: &SeedDocument, script: &Script) -> Result<Transaction, String> {
        self.sync_buyer_wallet()?;
        let fee_rate = self.pay_fee_rate().map_err(|err| format!("{err:?}"))?;
        let mut psbt = {
            let mut wallet = self.buyer_wallet();
            let untrusted = wallet.untrusted_pending();
            let mut builder = wallet.wallet.build_tx();
            builder.add_recipient(script.to_owned(), Amount::from_sat(head.terms.fee_sats)).fee_rate(fee_rate);
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

    fn set_fee_state(&self, state: FeeState) {
        *self.directory_fee.lock().unwrap_or_else(|err| err.into_inner()) = Some(state);
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
        let Ok(address) = store_fee_address(&head.terms.fee_key, store.id.as_bytes(), self.network.bitcoin()) else {
            return unavailable("The directory's fee key is not a public key.");
        };
        let state = self.directory_fee.lock().unwrap_or_else(|err| err.into_inner()).clone();
        let (state, until, error) = match state {
            Some(FeeState::Listed { until }) => ("listed", Some(until), None),
            Some(FeeState::Confirming) => ("confirming", None, None),
            Some(FeeState::Unpaid { error }) => ("unpaid", None, error),
            None => ("checking", None, None),
        };
        json!({
            "available": true,
            "address": address.to_string(),
            "sats": head.terms.fee_sats,
            "confirmations": head.terms.fee_confs,
            "epochBlocks": head.terms.epoch_blocks,
            "state": state,
            "listedUntilHeight": until,
            "error": error,
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
