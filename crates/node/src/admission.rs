//! The directory fee.
//!
//! The seller's node pays `fee_sats` from its own wallet to the fee key the
//! head names, with an OP_RETURN of `OBAD` and the store id. After
//! `fee_confs` confirmations inside `epoch_blocks` of the tip, the store key
//! signs an admission over that outpoint. The fee is a cost. It is not the
//! reputation bond and it is not a sale: the transaction has no order and no
//! buyer key.

use std::collections::BTreeSet;

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::script::PushBytesBuf;
use bitcoin::secp256k1::{Secp256k1, XOnlyPublicKey};
use bitcoin::{Address, Amount, Network, ScriptBuf, Transaction, Txid};

use openbazaar_protocol::{Admission, SeedDocument};

use crate::chain::ChainSource;
use crate::network::NetworkMode;
use crate::store_profile::Role;
use crate::Node;

pub(crate) fn fee_script(fee_key: &[u8; 32], network: Network) -> Result<ScriptBuf, String> {
    let key = XOnlyPublicKey::from_slice(fee_key).map_err(|_| "the head's fee key is not a public key".to_string())?;
    Ok(Address::p2tr(&Secp256k1::new(), key, None, network).script_pubkey())
}

pub(crate) fn fee_return(store_id: &[u8; 32]) -> Result<ScriptBuf, String> {
    let mut payload = Vec::with_capacity(36);
    payload.extend_from_slice(b"OBAD");
    payload.extend_from_slice(store_id);
    let push = PushBytesBuf::try_from(payload).map_err(|_| "the admission label does not fit an output".to_string())?;
    Ok(ScriptBuf::new_op_return(&push))
}

/// The payment proves the store id, a real output of at least `fee_sats` on
/// the head's fee key, `fee_confs` confirmations, and a height inside
/// `epoch_blocks` of the tip. It does not prove honesty, direct pay, or the bond.
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
    let Ok(script) = fee_script(&head.terms.fee_key, network.bitcoin()) else { return false };
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

impl Node {
    /// Pay the head's fee from this node's wallet, and queue an admission once
    /// it has the confirmations the head names. A missing head, mainnet, or
    /// a node that has not chosen to sell does nothing: a buyer's wallet
    /// never pays this. No new head sequence is signed.
    pub(crate) fn publish_admission(&self) -> Result<(), String> {
        if self.network != NetworkMode::Testnet4 || self.role() != Some(Role::Seller) {
            return Ok(());
        }
        let head = {
            let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
            replica.head()
        };
        let Some(head) = head else { return Ok(()) };
        let store = self.own_record().ok_or_else(|| "the own store has no signed record yet".to_string())?;
        let id = *store.id.as_bytes();
        if let Some(bytes) = self.read_outbox("admission") {
            if let Ok(admission) = Admission::decode(&bytes) {
                if admission.store_id == id && fee_confirmed(self.chain.as_ref(), self.network, &head, &admission) {
                    return Ok(());
                }
            }
        }
        let tx = match self.read_outbox("fee").and_then(|bytes| deserialize::<Transaction>(&bytes).ok()) {
            Some(tx) => tx,
            None => self.broadcast_fee(&head, &id)?,
        };
        let Some((_, Some(height))) = self.chain.transaction(&tx.compute_txid()).map_err(|err| err)? else {
            return Ok(());
        };
        let script = fee_script(&head.terms.fee_key, self.network.bitcoin())?;
        let Some(vout) = tx.output.iter().position(|output| output.script_pubkey == script) else {
            return Err("the fee transaction does not pay the head".into());
        };
        let admission = Admission::sign(id, tx.compute_txid().to_byte_array(), vout as u32, height, &self.own_signing_key().ok_or_else(|| "unlock the wallet to sign the admission".to_string())?)
            .map_err(|err| err.to_string())?;
        if !fee_confirmed(self.chain.as_ref(), self.network, &head, &admission) {
            return Ok(());
        }
        let replica = self.seed_replica.lock().unwrap_or_else(|err| err.into_inner());
        replica.queue_admission(&admission.encode().map_err(|err| err.to_string())?)?;
        replica.queue_store(&store)?;
        Ok(())
    }

    fn broadcast_fee(&self, head: &SeedDocument, store_id: &[u8; 32]) -> Result<Transaction, String> {
        self.sync_buyer_wallet()?;
        let fee_rate = self.pay_fee_rate().map_err(|err| format!("{err:?}"))?;
        let address = {
            let key = XOnlyPublicKey::from_slice(&head.terms.fee_key).map_err(|_| "the head's fee key is not a public key")?;
            Address::p2tr(&Secp256k1::new(), key, None, self.network.bitcoin())
        };
        let label = fee_return(store_id)?;
        let mut psbt = {
            let mut wallet = self.buyer_wallet();
            let untrusted = wallet.untrusted_pending();
            let mut builder = wallet.wallet.build_tx();
            builder
                .add_recipient(address.script_pubkey(), Amount::from_sat(head.terms.fee_sats))
                .add_recipient(label, Amount::ZERO)
                .fee_rate(fee_rate);
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

    fn read_outbox(&self, name: &str) -> Option<Vec<u8>> {
        self.seed_replica.lock().unwrap_or_else(|err| err.into_inner()).read_outbox(name)
    }

    /// The directory-fee transaction this node already broadcast, if it has one.
    pub(crate) fn directory_fee_txid(&self) -> Option<String> {
        let bytes = self.read_outbox("fee")?;
        let tx: Transaction = deserialize(&bytes).ok()?;
        Some(tx.compute_txid().to_string())
    }
}
