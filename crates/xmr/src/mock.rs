//! An in-memory Monero chain for offline tests.
//!
//! Blocks are real `Block`s whose miner transactions can pay any address,
//! and transactions the wallet signs are mined as they are. The chain starts
//! with [`HISTORY`] blocks of one output each so decoy selection has
//! something to pick from; those outputs are made-up points.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use curve25519_dalek::constants::ED25519_BASEPOINT_TABLE;
use monero_wallet::address::MoneroAddress;
use monero_wallet::block::{Block, BlockHeader};
use monero_wallet::ed25519::{Point, Scalar};
use monero_wallet::interface::{FeeRate, ScannableBlock};
use monero_wallet::primitives::keccak256;
use monero_wallet::transaction::{Input, Output, Pruned, Timelock, Transaction, TransactionPrefix};
use monero_wallet::{OutputWithDecoys, WalletOutput};
use rand_core::OsRng;

use crate::backend::BackendStatus;
use crate::decoys::DecoySource;
use crate::{XmrChain, XmrNetwork};

/// Blocks before the first one a test mines.
pub const HISTORY: u64 = 2000;

struct State {
    blocks: Vec<Block>,
    /// Non-miner transactions of each block.
    txs: Vec<Vec<Transaction>>,
    /// Cumulative RingCT outputs at the end of each block.
    distribution: Vec<u64>,
    known: HashMap<u64, [Point; 2]>,
    pool: Vec<Transaction>,
    key_images: HashSet<[u8; 32]>,
    published: Vec<Transaction>,
    offline: bool,
    fee_per_weight: u64,
}

pub struct MockChain {
    network: XmrNetwork,
    state: Mutex<State>,
}

impl MockChain {
    pub fn new(network: XmrNetwork) -> Self {
        let chain = Self {
            network,
            state: Mutex::new(State {
                blocks: Vec::new(),
                txs: Vec::new(),
                distribution: Vec::new(),
                known: HashMap::new(),
                pool: Vec::new(),
                key_images: HashSet::new(),
                published: Vec::new(),
                offline: false,
                fee_per_weight: 20_000,
            }),
        };
        {
            let mut state = chain.state.lock().unwrap();
            let filler = Output { amount: Some(0), key: Point::from(curve25519_dalek::constants::ED25519_BASEPOINT_POINT).compress(), view_tag: Some(0) };
            for _ in 0..HISTORY {
                state.push_block(vec![filler.clone()], Vec::new(), Vec::new());
            }
        }
        chain
    }

    pub fn height(&self) -> u64 {
        self.state.lock().unwrap().blocks.len() as u64 - 1
    }

    /// Mine one block whose miner transaction pays `amount` to `address`. Returns its txid.
    pub fn mine_to(&self, address: &MoneroAddress, amount: u64) -> [u8; 32] {
        let (output, extra) = pay(address, amount);
        let mut state = self.state.lock().unwrap();
        let pool = std::mem::take(&mut state.pool);
        state.push_block(vec![output], extra, pool);
        state.blocks.last().unwrap().miner_transaction().hash()
    }

    /// Mine `count` blocks, the first one with whatever is in the pool.
    pub fn mine(&self, count: u64) {
        let mut state = self.state.lock().unwrap();
        for _ in 0..count {
            let pool = std::mem::take(&mut state.pool);
            let output = Output { amount: Some(0), key: random_point().compress(), view_tag: Some(0) };
            state.push_block(vec![output], Vec::new(), pool);
        }
    }

    /// Replace the last `depth` blocks with `depth + 1` empty ones; their transactions go back to the pool.
    pub fn reorg(&self, depth: u64) {
        let mut state = self.state.lock().unwrap();
        for _ in 0..depth {
            state.blocks.pop();
            let txs = state.txs.pop().unwrap();
            state.distribution.pop();
            state.pool.extend(txs);
        }
        drop(state);
        let pool = std::mem::take(&mut self.state.lock().unwrap().pool);
        self.mine(depth + 1);
        self.state.lock().unwrap().pool = pool;
    }

    pub fn published(&self) -> Vec<Transaction> {
        self.state.lock().unwrap().published.clone()
    }

    pub fn set_offline(&self, offline: bool) {
        self.state.lock().unwrap().offline = offline;
    }

    pub fn set_fee_per_weight(&self, fee: u64) {
        self.state.lock().unwrap().fee_per_weight = fee;
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>, String> {
        let state = self.state.lock().unwrap();
        if state.offline {
            return Err("the mock Monero node is offline".into());
        }
        Ok(state)
    }
}

impl State {
    fn push_block(&mut self, outputs: Vec<Output>, extra: Vec<u8>, txs: Vec<Transaction>) {
        let number = self.blocks.len();
        let first = self.distribution.last().copied().unwrap_or(0);
        let mut index = first;
        for output in &outputs {
            if let (Some(key), Some(amount)) = (output.key.decompress(), output.amount) {
                if amount > 0 {
                    self.known.insert(index, [key, monero_wallet::ed25519::Commitment::new(Scalar::from(curve25519_dalek::Scalar::ONE), amount).commit()]);
                }
            }
            index += 1;
        }
        for tx in &txs {
            let Transaction::V2 { prefix, proofs: Some(proofs) } = tx else { continue };
            for (output, commitment) in prefix.outputs.iter().zip(&proofs.base.commitments) {
                if let (Some(key), Some(commitment)) = (output.key.decompress(), commitment.decompress()) {
                    self.known.insert(index, [key, commitment]);
                }
                index += 1;
            }
        }
        let miner = Transaction::V2 {
            prefix: TransactionPrefix { additional_timelock: Timelock::None, inputs: vec![Input::Gen(number)], outputs, extra },
            proofs: None,
        };
        let header = BlockHeader {
            hardfork_version: 16,
            hardfork_signal: 16,
            timestamp: 1_700_000_000 + 120 * number as u64,
            previous: self.blocks.last().map_or([0; 32], Block::hash),
            nonce: 0,
        };
        let block = Block::new(header, miner, txs.iter().map(Transaction::hash).collect()).expect("a miner transaction with one Gen input");
        self.blocks.push(block);
        self.txs.push(txs);
        self.distribution.push(index);
    }

    fn scannable(&self, number: usize) -> ScannableBlock {
        let first = if number == 0 { 0 } else { self.distribution[number - 1] };
        ScannableBlock {
            block: self.blocks[number].clone(),
            transactions: self.txs[number].iter().cloned().map(Transaction::<Pruned>::from).collect(),
            output_index_for_first_ringct_output: Some(first),
        }
    }
}

/// A miner output paying `amount` to `address`, and the tx extra with its public key.
fn pay(address: &MoneroAddress, amount: u64) -> (Output, Vec<u8>) {
    let r = Scalar::random(&mut OsRng).into();
    let spend = address.spend().into();
    let view = address.view().into();
    let tx_key = if address.is_subaddress() { r * spend } else { &r * ED25519_BASEPOINT_TABLE };
    let mut derivation = (r * view).mul_by_cofactor().compress().to_bytes().to_vec();
    derivation.push(0); // varint(output index 0)
    let view_tag = keccak256([b"view_tag".as_slice(), &derivation].concat())[0];
    let shared = Scalar::hash(&derivation).into();
    let key = Point::from(&shared * ED25519_BASEPOINT_TABLE + spend).compress();
    let mut extra = vec![1u8];
    extra.extend_from_slice(&Point::from(tx_key).compress().to_bytes());
    (Output { amount: Some(amount), key, view_tag: Some(view_tag) }, extra)
}

use crate::decoys::random_point;

impl XmrChain for MockChain {
    fn status(&self) -> BackendStatus {
        let config = crate::DaemonConfig::new(self.network, &["http://127.0.0.1:18081".to_string()], None).ok();
        BackendStatus { config, network: self.network, tip: Some(self.height()), active: None, error: None }
    }

    fn tip(&self) -> Result<u64, String> {
        Ok(self.state()?.blocks.len() as u64 - 1)
    }

    fn block_hash(&self, number: u64) -> Result<[u8; 32], String> {
        self.state()?.blocks.get(number as usize).map(Block::hash).ok_or_else(|| format!("no block {number}"))
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<ScannableBlock>, String> {
        let state = self.state()?;
        if to as usize >= state.blocks.len() || from > to {
            return Err(format!("no blocks {from}..={to}"));
        }
        Ok((from as usize..=to as usize).map(|number| state.scannable(number)).collect())
    }

    fn fee_rate(&self) -> Result<FeeRate, String> {
        Ok(FeeRate::new(self.state()?.fee_per_weight, 10_000).expect("nonzero"))
    }

    fn decoys(&self, outputs: Vec<WalletOutput>, height: u64) -> Result<Vec<OutputWithDecoys>, String> {
        let source = {
            let state = self.state()?;
            DecoySource { distribution: state.distribution.clone(), known: state.known.clone() }
        };
        source.select(outputs, height as usize)
    }

    fn publish(&self, tx: &Transaction) -> Result<(), String> {
        let mut state = self.state()?;
        let images: Vec<[u8; 32]> = tx
            .prefix()
            .inputs
            .iter()
            .filter_map(|input| match input {
                Input::ToKey { key_image, .. } => Some(key_image.to_bytes()),
                Input::Gen(_) => None,
            })
            .collect();
        if images.is_empty() || images.iter().any(|image| state.key_images.contains(image)) {
            return Err("the Monero network refused the transaction: double spend".into());
        }
        state.key_images.extend(images);
        state.pool.push(tx.clone());
        state.published.push(tx.clone());
        Ok(())
    }
}