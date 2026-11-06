//! The filter index on disk, sealed with the node's vault.
//!
//! `filters/state.obv` holds the followed scripts, matched transactions, and
//! recent headers as JSON. `filters/commitments.obv` holds one
//! `(height, block hash, filter hash)` record per checked block. Both reveal
//! which addresses are the user's or when the wallet started, so both are sealed.

use std::fs;
use std::path::PathBuf;

use bdk_wallet::chain::BlockId;
use bitcoin::bip158::FilterHash;
use bitcoin::block::Header;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Network, ScriptBuf, Transaction};
use serde_json::{json, Value};

use super::index::{FilterIndex, MatchedBlock};
use crate::vault::Vault;

const VERSION: u64 = 1;
const COMMITMENT_LEN: usize = 4 + 32 + 32;

/// Where a chain backend may keep sealed state. Handed over by [`crate::Node`] at open.
pub struct ChainStore {
    pub(crate) dir: PathBuf,
    pub(crate) vault: Vault,
    /// The newest block the node wallet has synced to, if any.
    pub(crate) wallet_tip: Option<BlockId>,
}

impl ChainStore {
    fn state_path(&self) -> PathBuf {
        self.dir.join("state.obv")
    }

    fn commitments_path(&self) -> PathBuf {
        self.dir.join("commitments.obv")
    }

    pub(crate) fn load(&self, network: Network) -> Result<Option<FilterIndex>, String> {
        let state = self.vault.read_sealed(&self.state_path()).map_err(|err| err.to_string())?;
        let Some(state) = state else {
            return Ok(None);
        };
        let commitments = self
            .vault
            .read_sealed(&self.commitments_path())
            .map_err(|err| err.to_string())?
            .unwrap_or_default();
        decode(network, &state, &commitments).map(Some)
    }

    pub(crate) fn save(&self, index: &FilterIndex) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|err| err.to_string())?;
        let (state, commitments) = encode(index);
        self.vault
            .write_sealed(&self.commitments_path(), &commitments)
            .and_then(|()| self.vault.write_sealed(&self.state_path(), &state))
            .map_err(|err| err.to_string())
    }
}

pub(crate) fn encode(index: &FilterIndex) -> (Vec<u8>, Vec<u8>) {
    let (pending_scripts, pending_from) = index.pending();
    let state = json!({
        "version": VERSION,
        "network": index.network.to_core_arg(),
        "start": block_id(index.start),
        "processed": block_id(index.processed),
        "caughtUp": index.caught_up,
        "headers": index.headers.iter().map(|(height, header)| json!([height, hex(&serialize(header))])).collect::<Vec<_>>(),
        "scripts": index.scripts.keys().map(|script| hex(script.as_bytes())).collect::<Vec<_>>(),
        "pendingScripts": pending_scripts.iter().map(|script| hex(script.as_bytes())).collect::<Vec<_>>(),
        "pendingFrom": pending_from,
        "blocks": index.blocks.iter().map(|(height, block)| json!({
            "height": height,
            "hash": block.hash.to_string(),
            "time": block.time,
            "txs": block.txs.iter().map(|tx| hex(&serialize(tx))).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "mempool": index.mempool.values().map(|(tx, seen)| json!([hex(&serialize(tx)), seen])).collect::<Vec<_>>(),
    });
    let mut commitments = Vec::with_capacity(index.commitments.len() * COMMITMENT_LEN);
    for (height, (hash, filter)) in &index.commitments {
        commitments.extend_from_slice(&height.to_le_bytes());
        commitments.extend_from_slice(hash.as_byte_array());
        commitments.extend_from_slice(filter.as_byte_array());
    }
    (serde_json::to_vec(&state).expect("json values serialize"), commitments)
}

pub(crate) fn decode(network: Network, state: &[u8], commitments: &[u8]) -> Result<FilterIndex, String> {
    let bad = |what: &str| format!("the filter index could not be read ({what})");
    let value: Value = serde_json::from_slice(state).map_err(|_| bad("json"))?;
    if value["version"].as_u64() != Some(VERSION) {
        return Err(bad("version"));
    }
    if value["network"].as_str() != Some(network.to_core_arg()) {
        return Err(bad("it belongs to another network"));
    }
    let mut index = FilterIndex::new(network, read_block_id(&value["start"]).ok_or_else(|| bad("start"))?);
    index.processed = read_block_id(&value["processed"]).ok_or_else(|| bad("processed"))?;
    index.caught_up = value["caughtUp"].as_bool().unwrap_or(false);
    for entry in value["headers"].as_array().ok_or_else(|| bad("headers"))? {
        let height = entry[0].as_u64().and_then(|h| u32::try_from(h).ok()).ok_or_else(|| bad("header height"))?;
        let header: Header = read_hex(&entry[1]).ok_or_else(|| bad("header"))?;
        index.headers.insert(height, header);
    }
    for script in value["scripts"].as_array().ok_or_else(|| bad("scripts"))? {
        index.scripts.insert(read_script(script).ok_or_else(|| bad("script"))?, 0);
    }
    for entry in value["blocks"].as_array().ok_or_else(|| bad("blocks"))? {
        let height = entry["height"].as_u64().and_then(|h| u32::try_from(h).ok()).ok_or_else(|| bad("block height"))?;
        let hash: BlockHash = entry["hash"].as_str().and_then(|h| h.parse().ok()).ok_or_else(|| bad("block hash"))?;
        let time = entry["time"].as_u64().and_then(|t| u32::try_from(t).ok()).ok_or_else(|| bad("block time"))?;
        let txs = entry["txs"]
            .as_array()
            .ok_or_else(|| bad("block txs"))?
            .iter()
            .map(|tx| read_hex::<Transaction>(tx))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| bad("block tx"))?;
        index.blocks.insert(height, MatchedBlock { hash, time, txs });
    }
    for entry in value["mempool"].as_array().ok_or_else(|| bad("mempool"))? {
        let tx: Transaction = read_hex(&entry[0]).ok_or_else(|| bad("mempool tx"))?;
        index.mempool.insert(tx.compute_txid(), (tx, entry[1].as_u64().unwrap_or(0)));
    }
    if commitments.len() % COMMITMENT_LEN != 0 {
        return Err(bad("commitments"));
    }
    for record in commitments.chunks_exact(COMMITMENT_LEN) {
        let height = u32::from_le_bytes(record[..4].try_into().expect("4 bytes"));
        let hash = BlockHash::from_byte_array(record[4..36].try_into().expect("32 bytes"));
        let filter = FilterHash::from_byte_array(record[36..].try_into().expect("32 bytes"));
        index.commitments.insert(height, (hash, filter));
    }
    let pending = value["pendingScripts"]
        .as_array()
        .map(|list| list.iter().filter_map(read_script).collect())
        .unwrap_or_default();
    let from = value["pendingFrom"].as_u64().and_then(|h| u32::try_from(h).ok());
    index.restore_pending(pending, from);
    Ok(index)
}

fn block_id(block: BlockId) -> Value {
    json!([block.height, block.hash.to_string()])
}

fn read_block_id(value: &Value) -> Option<BlockId> {
    Some(BlockId {
        height: u32::try_from(value[0].as_u64()?).ok()?,
        hash: value[1].as_str()?.parse().ok()?,
    })
}

fn read_script(value: &Value) -> Option<ScriptBuf> {
    unhex(value.as_str()?).map(ScriptBuf::from_bytes)
}

fn read_hex<T: bitcoin::consensus::Decodable>(value: &Value) -> Option<T> {
    deserialize(&unhex(value.as_str()?)?).ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}
