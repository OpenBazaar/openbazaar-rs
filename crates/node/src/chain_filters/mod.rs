//! Chain data from BIP 157/158 compact block filters over the Bitcoin P2P network.
//!
//! Peers send block headers and one filter per block. The node tests each
//! filter against its own scripts on this machine and downloads only the
//! blocks that match, each from a random connected peer. A peer learns
//! which whole blocks the node fetched, never which scripts or addresses it
//! follows. Broadcast hands the transaction to one random peer.
//!
//! The client is [`bip157`] (Kyoto). With [`FilterConfig::proxy`] every
//! peer connection, DNS seeding included, goes through that SOCKS5 proxy (Tor).

mod driver;
pub(crate) mod index;
pub(crate) mod store;
#[cfg(test)]
mod mock_peer;
#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bdk_wallet::chain::BlockId;
use bdk_wallet::{KeychainKind, Update};
use bitcoin::constants::genesis_block;
use bitcoin::{Address, BlockHash, Network, OutPoint, Script, ScriptBuf, Transaction, TxOut};
use tokio::sync::Notify;

use crate::buyer_orders::now_secs;
use crate::chain::{AddressTx, ChainSource, ChainStatus, WalletRequest};
use index::FilterIndex;
pub use store::ChainStore;

/// Scripts taken from each keychain of a full wallet scan.
const FULL_SCAN_SPKS: usize = 100;
const BROADCAST_TIMEOUT: Duration = Duration::from_secs(30);
const FEE_BLOCKS: u32 = 3;
const FEE_CACHE: Duration = Duration::from_secs(30 * 60);
const PEER_INFO_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterConfig {
    pub network: Network,
    /// `(host, port)` peers to try first; with `only_these_peers`, the only ones.
    pub peers: Vec<(String, u16)>,
    /// Never use DNS seeds or gossiped addresses. Set for the user's own node.
    pub only_these_peers: bool,
    pub required_peers: u8,
    /// SOCKS5 proxy (Tor) for every peer connection, broadcast included.
    pub proxy: Option<SocketAddr>,
}

impl FilterConfig {
    /// Peers found through DNS seeds and gossip. Few testnet4 nodes serve
    /// filters, so waiting for a second one there can stall sync for hours.
    pub fn public(network: Network) -> Self {
        Self {
            network,
            peers: Vec::new(),
            only_these_peers: false,
            required_peers: if network == Network::Bitcoin { 2 } else { 1 },
            proxy: None,
        }
    }

    /// Only the user's own node(s). They must serve filters
    /// (`blockfilterindex=1` and `peerblockfilters=1` in bitcoind).
    pub fn own_node(network: Network, peers: Vec<(String, u16)>) -> Self {
        Self {
            network,
            required_peers: 1,
            peers,
            only_these_peers: true,
            proxy: None,
        }
    }
}

/// A recent block on each network. A wallet that has never synced starts its scan after it.
pub(crate) fn default_start(network: Network) -> BlockId {
    let (height, hash) = match network {
        Network::Testnet4 => (159_000, "00000000055dc440aea31554f958ec7dea1ed7c466272b063c51ddd029eea8a0"),
        Network::Bitcoin => (974_000, "00000000000000000000d7d13f5ac024052cad9613d9a6608f76df081d74e865"),
        _ => return BlockId { height: 0, hash: genesis_block(network).block_hash() },
    };
    BlockId { height, hash: hash.parse().expect("checkpoint hashes are valid") }
}

pub(crate) struct Shared {
    index: Mutex<FilterIndex>,
    store: Mutex<Option<ChainStore>>,
    requester: Mutex<Option<bip157::Requester>>,
    last_contact: Mutex<Option<Instant>>,
    last_error: Mutex<Option<String>>,
    fee: Mutex<Option<(f64, Instant)>>,
    wake: Notify,
    stop: AtomicBool,
}

impl Shared {
    fn index(&self) -> MutexGuard<'_, FilterIndex> {
        self.index.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn contact(&self) {
        *self.last_contact.lock().unwrap_or_else(|err| err.into_inner()) = Some(Instant::now());
    }

    fn set_error(&self, message: String) {
        *self.last_error.lock().unwrap_or_else(|err| err.into_inner()) = Some(message);
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn save(&self) {
        let store = self.store.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(store) = store.as_ref() {
            if let Err(err) = store.save(&self.index()) {
                eprintln!("ob-node: saving the filter index: {err}");
            }
        }
    }

    fn requester(&self) -> Result<bip157::Requester, String> {
        self.requester
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or_else(|| "the filter client is not connected to any peer yet".to_string())
    }
}

/// Whether a scan that is behind has stayed at `height` for longer than `after`.
fn stalled(progress: &mut Option<(u32, Instant)>, height: u32, caught_up: bool, now: Instant, after: Duration) -> bool {
    match *progress {
        Some((seen, since)) if seen == height => !caught_up && now.duration_since(since) > after,
        _ => {
            *progress = Some((height, now));
            false
        }
    }
}

/// The compact block filter backend. Nothing runs until [`ChainSource::open`].
pub struct FilterChain {
    runtime: tokio::runtime::Runtime,
    config: FilterConfig,
    shared: Arc<Shared>,
    started: Mutex<Option<Instant>>,
    /// The scanned height last seen, and when it was first seen.
    progress: Mutex<Option<(u32, Instant)>>,
}

impl FilterChain {
    pub fn new(config: FilterConfig) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ob-filters")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let start = default_start(config.network);
        Ok(Self {
            runtime,
            shared: Arc::new(Shared {
                index: Mutex::new(FilterIndex::new(config.network, start)),
                store: Mutex::new(None),
                requester: Mutex::new(None),
                last_contact: Mutex::new(None),
                last_error: Mutex::new(None),
                fee: Mutex::new(None),
                wake: Notify::new(),
                stop: AtomicBool::new(false),
            }),
            config,
            started: Mutex::new(None),
            progress: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &FilterConfig {
        &self.config
    }

    /// No peer has answered for `after` since the client started or last
    /// heard from one, or the scan is behind and has not moved for `after`.
    /// A peer can answer a handshake and still never serve the filters.
    pub(crate) fn unreachable_for(&self, after: Duration) -> bool {
        let Some(started) = *self.started.lock().unwrap_or_else(|err| err.into_inner()) else {
            return false;
        };
        let last = self.shared.last_contact.lock().unwrap_or_else(|err| err.into_inner()).unwrap_or(started);
        let (height, caught_up) = {
            let index = self.shared.index();
            (index.processed.height, index.caught_up)
        };
        let mut progress = self.progress.lock().unwrap_or_else(|err| err.into_inner());
        last.elapsed() > after || stalled(&mut progress, height, caught_up, Instant::now(), after)
    }

    fn ready(&self, scripts: &[ScriptBuf]) -> Result<(), String> {
        let index = self.shared.index();
        index.tip()?;
        if scripts.iter().all(|script| index.is_ready(script)) {
            Ok(())
        } else {
            Err("compact block filters are still scanning for a newly followed address".into())
        }
    }

    fn watch(&self, scripts: Vec<ScriptBuf>) {
        if self.shared.index().watch(scripts) {
            self.shared.wake.notify_one();
        }
    }

    /// Run `future` on the client's runtime for at most `limit`.
    fn wait<F: std::future::Future>(&self, limit: Duration, future: F) -> Option<F::Output> {
        self.runtime.block_on(async { tokio::time::timeout(limit, future).await.ok() })
    }

    fn recent_fee_rate(&self) -> Result<f64, String> {
        let requester = self.shared.requester()?;
        let hashes: Vec<BlockHash> = {
            let index = self.shared.index();
            index.tip()?;
            index.snapshot().iter().rev().take(FEE_BLOCKS as usize).map(|(_, header)| header.block_hash()).collect()
        };
        let mut rates = Vec::new();
        for hash in hashes {
            if let Some(Ok(rate)) = self.wait(BROADCAST_TIMEOUT, requester.average_fee_rate(hash)) {
                rates.push(rate.to_sat_per_kwu() as f64 / 250.0);
            }
        }
        rates.sort_by(f64::total_cmp);
        rates
            .get(rates.len() / 2)
            .copied()
            .ok_or_else(|| "no recent block gave a fee estimate".to_string())
    }
}

impl Drop for FilterChain {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Ok(requester) = self.shared.requester() {
            let _ = requester.shutdown();
        }
        self.shared.save();
    }
}

impl ChainSource for FilterChain {
    fn open(&self, store: ChainStore) -> Result<(), String> {
        let mut started = self.started.lock().unwrap_or_else(|err| err.into_inner());
        if started.is_some() {
            return Ok(());
        }
        let index = match store.load(self.config.network)? {
            Some(index) => index,
            None => {
                let start = store.wallet_tip.filter(|tip| tip.height > 0).unwrap_or_else(|| default_start(self.config.network));
                FilterIndex::new(self.config.network, start)
            }
        };
        *self.shared.index() = index;
        *self.shared.store.lock().unwrap_or_else(|err| err.into_inner()) = Some(store);
        self.shared.save();
        self.runtime.spawn(driver::run(self.shared.clone(), self.config.clone()));
        *started = Some(Instant::now());
        Ok(())
    }

    fn status(&self) -> ChainStatus {
        let peers = self.shared.requester().ok().and_then(|requester| {
            self.wait(PEER_INFO_TIMEOUT, requester.peer_info()).and_then(Result::ok).map(|peers| peers.len())
        });
        let index = self.shared.index();
        let own = self.config.only_these_peers;
        ChainStatus {
            backend: if own { "own-node" } else { "compact-filters" },
            privacy: if own { "own-node" } else { "private" },
            reveals: if own {
                "Your own Bitcoin node serves headers, filters, and matched blocks. No one else is asked.".into()
            } else {
                "Peers see this machine's IP address and which whole blocks it downloads, not which addresses are yours. Broadcasts go to one random peer.".into()
            },
            peers,
            scanned_height: Some(index.processed.height),
            syncing: !index.caught_up,
            error: self.shared.last_error.lock().unwrap_or_else(|err| err.into_inner()).clone(),
            fallback: None,
            fallback_active: false,
        }
    }

    fn tip_height(&self) -> Result<u32, String> {
        self.shared.index().tip()
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        let script = address.script_pubkey();
        self.watch(vec![script.clone()]);
        self.ready(std::slice::from_ref(&script))?;
        Ok(self.shared.index().address_txs(&script))
    }

    fn script_txs(&self, script: &Script, from: u32) -> Result<Vec<AddressTx>, String> {
        let script = script.to_owned();
        if self.shared.index().watch_from([script.clone()], from) {
            self.shared.wake.notify_one();
        }
        self.ready(std::slice::from_ref(&script))?;
        Ok(self.shared.index().address_txs(&script))
    }

    fn fee_rate(&self) -> Result<f64, String> {
        if let Some((rate, at)) = *self.shared.fee.lock().unwrap_or_else(|err| err.into_inner()) {
            if at.elapsed() < FEE_CACHE {
                return Ok(rate);
            }
        }
        let rate = self.recent_fee_rate()?;
        *self.shared.fee.lock().unwrap_or_else(|err| err.into_inner()) = Some((rate, Instant::now()));
        Ok(rate)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        let requester = self.shared.requester()?;
        self.wait(BROADCAST_TIMEOUT, requester.submit_package(tx.clone()))
            .ok_or_else(|| "no peer took the transaction in time".to_string())?
            .map_err(|err| err.to_string())?;
        self.shared.index().add_mempool(tx.clone(), now_secs());
        self.shared.save();
        Ok(())
    }

    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        let spks: Vec<(Option<(KeychainKind, u32)>, ScriptBuf)> = match request {
            WalletRequest::Full(mut request) => {
                let mut spks = Vec::new();
                for keychain in request.keychains() {
                    spks.extend(
                        request
                            .iter_spks(keychain)
                            .take(FULL_SCAN_SPKS)
                            .map(|(index, script)| (Some((keychain, index)), script)),
                    );
                }
                spks
            }
            WalletRequest::Sync(mut request) => {
                request.iter_spks_with_expected_txids().map(|item| (None, item.spk)).collect()
            }
        };
        let scripts: Vec<ScriptBuf> = spks.iter().map(|(_, script)| script.clone()).collect();
        self.watch(scripts.clone());
        self.ready(&scripts)?;
        Ok(self.shared.index().wallet_update(&spks))
    }

    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String> {
        self.shared.index().tip()?;
        self.shared.index().output(outpoint).map(Some).ok_or_else(|| {
            "compact block filters find transactions by address; this output does not pay an address the node follows".to_string()
        })
    }
}
