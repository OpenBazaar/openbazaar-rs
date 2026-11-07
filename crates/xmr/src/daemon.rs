//! [`XmrChain`] over monerod's RPC, through [`RpcTransport`].
//!
//! Nodes are tried in the order given; a node that fails is skipped until the
//! others fail too. Each node must have this network's genesis block. When
//! there are two or more nodes, a payment is broadcast through a different
//! node than the one that supplied its ring members.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use monero_daemon_rpc::MoneroDaemon;
use monero_wallet::interface::{
    FeePriority, FeeRate, ProvidesBlockchain, ProvidesBlockchainMeta, ProvidesFeeRates, ProvidesScannableBlocks,
    PublishTransaction, PublishTransactionError, ScannableBlock,
};
use monero_wallet::transaction::Transaction;
use monero_wallet::{OutputWithDecoys, WalletOutput};
use rand_core::OsRng;

use crate::backend::DaemonConfig;
use crate::transport::RpcTransport;
use crate::{BackendStatus, XmrChain, RING_LEN};

/// Refuse fee rates above this many piconero per weight unit (about 50x today's normal rate).
const MAX_FEE_PER_WEIGHT: u64 = 1_000_000;

type Daemon = MoneroDaemon<RpcTransport>;

struct Slot {
    url: String,
    transport: RpcTransport,
    daemon: Mutex<Option<Daemon>>,
}

pub struct DaemonChain {
    config: DaemonConfig,
    runtime: tokio::runtime::Runtime,
    slots: Vec<Slot>,
    current: AtomicUsize,
    status: Mutex<(Option<u64>, Option<String>, Option<String>)>,
}

/// A node failed (try the next) or the request itself was refused (stop).
enum Failure {
    Node(String),
    Final(String),
}

impl DaemonChain {
    pub fn new(config: DaemonConfig) -> Result<Self, String> {
        for node in &config.nodes {
            if node.kind != crate::NodeKind::Own && config.proxy.is_none() {
                return Err(format!(
                    "{} is not contacted without --proxy; a remote Monero node would see this machine's IP",
                    node.url
                ));
            }
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("ob-xmr")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let slots = config
            .nodes
            .iter()
            .map(|node| {
                let proxy = config.proxied(node).then(|| config.proxy.clone()).flatten();
                Ok(Slot { url: node.url.clone(), transport: RpcTransport::new(&node.url, proxy.as_deref())?, daemon: Mutex::new(None) })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { config, runtime, slots, current: AtomicUsize::new(0), status: Mutex::new((None, None, None)) })
    }

    fn connect(&self, slot: &Slot) -> Result<Daemon, String> {
        if let Some(daemon) = slot.daemon.lock().unwrap().clone() {
            return Ok(daemon);
        }
        let network = self.config.network;
        let daemon = self.runtime.block_on(async {
            let daemon = MoneroDaemon::new(slot.transport.clone()).await.map_err(|err| err.to_string())?;
            let genesis = ProvidesBlockchain::block_hash(&daemon, 0).await.map_err(|err| err.to_string())?;
            if genesis != network.genesis_hash() {
                return Err(format!("{} is not a Monero {network} node", slot.url));
            }
            Ok::<_, String>(daemon)
        })?;
        *slot.daemon.lock().unwrap() = Some(daemon.clone());
        Ok(daemon)
    }

    /// Run `call` against the current node, then the others in order.
    /// `skip` starts that many nodes further on (for broadcasting elsewhere).
    fn call<R, F, Fut>(&self, skip: usize, call: F) -> Result<R, String>
    where
        F: Fn(Daemon) -> Fut,
        Fut: Future<Output = Result<R, Failure>>,
    {
        let count = self.slots.len();
        let start = self.current.load(Ordering::Relaxed) + skip.min(count - 1);
        let mut last = String::new();
        for offset in 0..count {
            let index = (start + offset) % count;
            let slot = &self.slots[index];
            let result = self.connect(slot).map_err(Failure::Node).and_then(|daemon| self.runtime.block_on(call(daemon)));
            match result {
                Ok(value) => {
                    if skip == 0 {
                        self.current.store(index, Ordering::Relaxed);
                    }
                    let mut status = self.status.lock().unwrap();
                    status.1 = Some(slot.url.clone());
                    status.2 = None;
                    return Ok(value);
                }
                Err(Failure::Final(err)) => return Err(err),
                Err(Failure::Node(err)) => {
                    *slot.daemon.lock().unwrap() = None;
                    last = format!("{}: {err}", slot.url);
                }
            }
        }
        self.status.lock().unwrap().2 = Some(last.clone());
        Err(last)
    }
}

fn node<E: std::fmt::Display>(err: E) -> Failure {
    Failure::Node(err.to_string())
}

impl XmrChain for DaemonChain {
    fn status(&self) -> BackendStatus {
        let (tip, active, error) = self.status.lock().unwrap().clone();
        BackendStatus { config: Some(self.config.clone()), network: self.config.network, tip, active, error }
    }

    fn tip(&self) -> Result<u64, String> {
        let tip = self.call(0, |daemon| async move { daemon.latest_block_number().await.map_err(node) })? as u64;
        self.status.lock().unwrap().0 = Some(tip);
        Ok(tip)
    }

    fn block_hash(&self, number: u64) -> Result<[u8; 32], String> {
        let number = usize::try_from(number).map_err(|err| err.to_string())?;
        self.call(0, |daemon| async move { ProvidesBlockchain::block_hash(&daemon, number).await.map_err(node) })
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<ScannableBlock>, String> {
        let range = usize::try_from(from).map_err(|err| err.to_string())?..=usize::try_from(to).map_err(|err| err.to_string())?;
        self.call(0, |daemon| {
            let range = range.clone();
            async move { daemon.contiguous_scannable_blocks(range).await.map_err(node) }
        })
    }

    fn fee_rate(&self) -> Result<FeeRate, String> {
        self.call(0, |daemon| async move {
            ProvidesFeeRates::fee_rate(&daemon, FeePriority::Normal, MAX_FEE_PER_WEIGHT).await.map_err(node)
        })
    }

    fn decoys(&self, outputs: Vec<WalletOutput>, height: u64) -> Result<Vec<OutputWithDecoys>, String> {
        let height = usize::try_from(height).map_err(|err| err.to_string())?;
        self.call(0, |daemon| {
            let outputs = outputs.clone();
            async move {
                let mut out = Vec::with_capacity(outputs.len());
                for output in outputs {
                    out.push(OutputWithDecoys::new(&mut OsRng, &daemon, RING_LEN, height, output).await.map_err(node)?);
                }
                Ok(out)
            }
        })
    }

    fn publish(&self, tx: &Transaction) -> Result<(), String> {
        self.call(1, |daemon| async move {
            match daemon.publish_transaction(tx).await {
                Ok(()) => Ok(()),
                Err(PublishTransactionError::TransactionRejected(why)) => {
                    Err(Failure::Final(format!("the Monero network refused the transaction: {why}")))
                }
                Err(err) => Err(node(err)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{DaemonConfig, DaemonNode};
    use crate::{NodeKind, XmrNetwork};

    #[test]
    fn a_public_stagenet_daemon_is_not_dialed_without_a_proxy() {
        let config = DaemonConfig {
            network: XmrNetwork::Stagenet,
            nodes: vec![DaemonNode { url: "http://203.0.113.9:38081".into(), kind: NodeKind::Public }],
            proxy: None,
        };
        let started = std::time::Instant::now();
        let err = match DaemonChain::new(config) {
            Err(err) => err,
            Ok(_) => panic!("the public daemon was accepted"),
        };
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "the daemon was dialed");
        assert!(err.contains("without --proxy"), "{err}");
    }
}
