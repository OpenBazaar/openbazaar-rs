//! Where the node learns about the chain.
//!
//! [`ChainSource`] is the whole surface: tip height, the transactions that
//! pay one address, a fee estimate, broadcast, and a BDK wallet update.
//! [`EsploraChain`] talks to an Esplora server (mempool.space by default).
//! Every query names addresses to that server; on mainnet the node prints a
//! warning when the server is public. Tests use their own implementation.

use bdk_esplora::esplora_client::{self, BlockingClient};
use bdk_esplora::EsploraExt;
use bdk_wallet::chain::spk_client::{FullScanRequest, SyncRequest};
use bdk_wallet::{KeychainKind, Update};
use bitcoin::{Address, Transaction, Txid};

const STOP_GAP: usize = 20;
const PARALLEL_REQUESTS: usize = 4;
const FEE_TARGET_BLOCKS: u16 = 6;

/// One transaction that pays an address, with the outputs that pay it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressTx {
    pub txid: Txid,
    /// `(vout, sats)` for each output to the watched address.
    pub outputs: Vec<(u32, u64)>,
    /// Block height, or `None` while the transaction is in the mempool.
    pub height: Option<u32>,
}

impl AddressTx {
    pub fn value(&self) -> u64 {
        self.outputs.iter().map(|(_, sats)| sats).sum()
    }
}

/// A BDK request built while the wallet is locked and run after it is released.
pub enum WalletRequest {
    Full(FullScanRequest<KeychainKind>),
    Sync(SyncRequest<(KeychainKind, u32)>),
}

pub trait ChainSource: Send + Sync {
    fn tip_height(&self) -> Result<u32, String>;
    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String>;
    /// Fee estimate in sat/vB for a few blocks out.
    fn fee_rate(&self) -> Result<f64, String>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), String>;
    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String>;
}

pub struct EsploraChain {
    client: BlockingClient,
}

impl EsploraChain {
    pub fn new(url: &str) -> Self {
        let client = esplora_client::Builder::new(url.trim_end_matches('/'))
            .timeout(20)
            .build_blocking();
        Self { client }
    }
}

impl ChainSource for EsploraChain {
    fn tip_height(&self) -> Result<u32, String> {
        self.client.get_height().map_err(|err| err.to_string())
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        let script = address.script_pubkey();
        let txs = self
            .client
            .get_address_txs(address, None)
            .map_err(|err| err.to_string())?;
        Ok(txs
            .into_iter()
            .map(|tx| AddressTx {
                txid: tx.txid,
                outputs: tx
                    .vout
                    .iter()
                    .enumerate()
                    .filter(|(_, out)| out.scriptpubkey == script)
                    .map(|(vout, out)| (vout as u32, out.value))
                    .collect(),
                height: if tx.status.confirmed { tx.status.block_height } else { None },
            })
            .filter(|tx| !tx.outputs.is_empty())
            .collect())
    }

    fn fee_rate(&self) -> Result<f64, String> {
        let estimates = self.client.get_fee_estimates().map_err(|err| err.to_string())?;
        esplora_client::convert_fee_rate(FEE_TARGET_BLOCKS as usize, estimates)
            .map(f64::from)
            .ok_or_else(|| "the server returned no fee estimate".to_string())
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        self.client.broadcast(tx).map_err(|err| err.to_string())
    }

    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        match request {
            WalletRequest::Full(request) => self
                .client
                .full_scan(request, STOP_GAP, PARALLEL_REQUESTS)
                .map(Update::from)
                .map_err(|err| err.to_string()),
            WalletRequest::Sync(request) => self
                .client
                .sync(request, PARALLEL_REQUESTS)
                .map(Update::from)
                .map_err(|err| err.to_string()),
        }
    }
}

/// No chain at all. Every query fails, so status stays where it is.
pub struct OfflineChain;

impl ChainSource for OfflineChain {
    fn tip_height(&self) -> Result<u32, String> {
        Err(offline())
    }
    fn address_txs(&self, _: &Address) -> Result<Vec<AddressTx>, String> {
        Err(offline())
    }
    fn fee_rate(&self) -> Result<f64, String> {
        Err(offline())
    }
    fn broadcast(&self, _: &Transaction) -> Result<(), String> {
        Err(offline())
    }
    fn wallet_update(&self, _: WalletRequest) -> Result<Update, String> {
        Err(offline())
    }
}

fn offline() -> String {
    "this node has no chain source".to_string()
}
