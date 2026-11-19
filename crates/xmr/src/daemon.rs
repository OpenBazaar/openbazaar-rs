//! [`XmrChain`] over monerod's RPC, through [`RpcTransport`].
//!
//! Nodes are tried in the order given. A node that fails is left alone for a
//! while, so one dead onion is not dialed again at the start of every scan.
//! Each node must have this network's genesis block. When there are two or
//! more nodes, a payment is broadcast through a different node than the one
//! that supplied its ring members.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use monero_daemon_rpc::MoneroDaemon;
use monero_wallet::interface::{
    FeePriority, FeeRate, ProvidesBlockchain, ProvidesBlockchainMeta, ProvidesFeeRates, ProvidesScannableBlocks,
    ProvidesTransactions, PublishTransaction, PublishTransactionError, ScannableBlock, TransactionsError,
};
use monero_wallet::transaction::{Pruned, Transaction};
use monero_wallet::{OutputWithDecoys, WalletOutput};
use rand_core::OsRng;

use crate::backend::DaemonConfig;
use crate::transport::RpcTransport;
use crate::{BackendStatus, XmrChain, RING_LEN};

/// Refuse fee rates above this many piconero per weight unit (about 50x today's normal rate).
const MAX_FEE_PER_WEIGHT: u64 = 1_000_000;
/// After a node fails, leave it out of the rotation for this long.
const SKIP_FOR: Duration = Duration::from_secs(90);
/// How long one node may sit on a request. A silent onion used to hold the
/// only scan thread until the process was restarted, so the other daemons
/// were never asked and no height was saved. A circuit that is going to
/// answer usually does so inside a minute.
const ATTEMPT: Duration = Duration::from_secs(60);
/// Blocks per `get_blocks.bin` request through Tor. Stagenet onion nodes
/// dropped 15-block requests partway through, while 4 at a time came through.
const TOR_BLOCKS_PER_REQUEST: u64 = 4;

type Daemon = MoneroDaemon<RpcTransport>;

struct Slot {
    url: String,
    transport: RpcTransport,
    daemon: Mutex<Option<Daemon>>,
    /// Do not dial this node again until then. A stall has to time out once;
    /// after that the wallet uses the other nodes.
    skip_until: Mutex<Option<Instant>>,
}

pub struct DaemonChain {
    config: DaemonConfig,
    runtime: tokio::runtime::Runtime,
    /// One request at a time. The runtime is current-thread, so two `block_on`
    /// calls would panic; the scan thread and a quote can both want a daemon.
    turn: Mutex<()>,
    slots: Vec<Slot>,
    current: AtomicUsize,
    status: Mutex<Seen>,
    /// How long one node may sit on a request before the next daemon is asked.
    attempt: Duration,
}

/// The tip, the node that last answered, the last error, and since when every node has failed.
#[derive(Default)]
struct Seen {
    tip: Option<u64>,
    active: Option<String>,
    error: Option<String>,
    failing_since: Option<Instant>,
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
        Self::open(config, ATTEMPT)
    }

    fn open(config: DaemonConfig, attempt: Duration) -> Result<Self, String> {
        // The calling thread drives the request. One shared worker left the
        // fetch parked and the scan waiting on it, so curl could finish the
        // same onion in a few seconds while the wallet never recorded a height.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|err| err.to_string())?;
        let slots = config
            .nodes
            .iter()
            .map(|node| {
                let proxy = config.proxied(node).then(|| config.proxy.clone()).flatten();
                Ok(Slot {
                    url: node.url.clone(),
                    transport: RpcTransport::bounded(&node.url, proxy.as_deref(), attempt, attempt)?,
                    daemon: Mutex::new(None),
                    skip_until: Mutex::new(None),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            config,
            runtime,
            turn: Mutex::new(()),
            slots,
            current: AtomicUsize::new(0),
            status: Mutex::new(Seen::default()),
            attempt,
        })
    }

    /// Run `fut` on this thread's runtime, and give the node up when `attempt`
    /// ends. The same thread polls the request and the timer, so a silent
    /// daemon cannot pin the scan the way a worker that never polls the task did.
    fn drive<T>(&self, fut: impl Future<Output = T>) -> Result<T, String> {
        let _turn = self.turn.lock().unwrap();
        self.runtime.block_on(async move {
            match tokio::time::timeout(self.attempt, fut).await {
                Ok(value) => Ok(value),
                Err(_) => Err("monerod: timed out".into()),
            }
        })
    }

    fn connect(&self, slot: &Slot) -> Result<Daemon, String> {
        if let Some(daemon) = slot.daemon.lock().unwrap().clone() {
            return Ok(daemon);
        }
        let network = self.config.network;
        let transport = slot.transport.clone();
        let url = slot.url.clone();
        let daemon = self.drive(async move {
            let daemon = MoneroDaemon::new(transport).await.map_err(|err| err.to_string())?;
            let genesis = ProvidesBlockchain::block_hash(&daemon, 0).await.map_err(|err| err.to_string())?;
            if genesis != network.genesis_hash() {
                return Err(format!("{url} is not a Monero {network} node"));
            }
            Ok::<_, String>(daemon)
        })??;
        *slot.daemon.lock().unwrap() = Some(daemon.clone());
        Ok(daemon)
    }

    fn ready(&self, index: usize) -> bool {
        self.slots[index].skip_until.lock().unwrap().is_none_or(|until| Instant::now() >= until)
    }

    /// Run `call` against the current node, then the others in order, skipping
    /// any that failed recently. `skip` starts that many nodes further on (for
    /// broadcasting elsewhere).
    fn call<R, F, Fut>(&self, skip: usize, call: F) -> Result<R, String>
    where
        F: Fn(Daemon) -> Fut,
        Fut: Future<Output = Result<R, Failure>>,
    {
        let count = self.slots.len();
        let start = self.current.load(Ordering::Relaxed) + skip.min(count.saturating_sub(1));
        let order = attempt_order(count, start, |index| self.ready(index));
        if order.is_empty() {
            let status = self.status.lock().unwrap();
            return Err(status.error.clone().unwrap_or_else(|| "every Monero node failed recently; trying again shortly".into()));
        }
        let mut failed = Vec::new();
        for index in order {
            let slot = &self.slots[index];
            let result = self.connect(slot).map_err(Failure::Node).and_then(|daemon| match self.drive(call(daemon)) {
                Ok(result) => result,
                Err(err) => Err(Failure::Node(err)),
            });
            match result {
                Ok(value) => {
                    *slot.skip_until.lock().unwrap() = None;
                    if skip == 0 {
                        self.current.store(index, Ordering::Relaxed);
                    }
                    let mut status = self.status.lock().unwrap();
                    status.active = Some(slot.url.clone());
                    status.error = None;
                    status.failing_since = None;
                    return Ok(value);
                }
                Err(Failure::Final(err)) => return Err(err),
                Err(Failure::Node(err)) => {
                    *slot.daemon.lock().unwrap() = None;
                    // The circuit that just failed is not used again. A reused
                    // SOCKS username keeps Tor on a stalled circuit.
                    slot.transport.rotate_circuit();
                    *slot.skip_until.lock().unwrap() = Some(Instant::now() + SKIP_FOR);
                    if skip == 0 {
                        self.current.store((index + 1) % count, Ordering::Relaxed);
                    }
                    failed.push(format!("{}: {err}", slot.url));
                }
            }
        }
        // One line, after every node in this attempt has failed. A single
        // dead onion is not the error while another daemon can still answer.
        let summary = format!("every Monero node failed ({})", failed.join("; "));
        let mut status = self.status.lock().unwrap();
        status.error = Some(summary.clone());
        status.failing_since.get_or_insert_with(Instant::now);
        Err(summary)
    }

    /// Run `call` against the current node only, and leave no trace if it
    /// fails: no backend error, no node rested. The pool is a nicety; its
    /// failure must not hold up the next block scan.
    fn quietly<R, F, Fut>(&self, call: F) -> Result<R, String>
    where
        F: FnOnce(Daemon) -> Fut,
        Fut: Future<Output = Result<R, String>>,
    {
        let index = self.current.load(Ordering::Relaxed) % self.slots.len();
        if !self.ready(index) {
            return Err(format!("{} failed recently", self.slots[index].url));
        }
        let daemon = self.connect(&self.slots[index])?;
        self.drive(call(daemon))?
    }
}

/// `get_transaction_pool_hashes` lists hashes only, about 70 bytes each.
const POOL_HASHES_LIMIT: usize = 2 * 1024 * 1024;

fn pool_hashes_from(text: &str) -> Result<Vec<[u8; 32]>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| "monerod sent an unreadable pool".to_string())?;
    // An empty pool has no `tx_hashes` at all.
    let Some(list) = value.get("tx_hashes") else { return Ok(Vec::new()) };
    list.as_array()
        .ok_or("monerod sent an unreadable pool")?
        .iter()
        .map(|hash| {
            let mut out = [0u8; 32];
            hex::decode_to_slice(hash.as_str().unwrap_or_default(), &mut out).map_err(|_| "monerod sent a bad pool hash".to_string())?;
            Ok(out)
        })
        .collect()
}

/// Indexes to dial, starting at `start`, leaving out nodes `ready` rejects.
/// Empty when every node is resting, so the caller does not dial the dead one again.
fn attempt_order(count: usize, start: usize, ready: impl Fn(usize) -> bool) -> Vec<usize> {
    (0..count).map(|offset| (start + offset) % count).filter(|index| ready(*index)).collect()
}

/// The error to show, and whether the nodes are still being reached.
///
/// An error is stored only after every node in the attempt failed, and it is
/// shown then. Before any node has answered, the backend is still connecting.
fn reported(seen: &Seen, _now: Instant) -> (Option<String>, bool) {
    match (&seen.error, &seen.active) {
        (Some(err), _) => (Some(err.clone()), false),
        (None, None) => (None, true),
        (None, Some(_)) => (None, false),
    }
}

/// `from..=to` split into inclusive ranges of at most `per` blocks.
fn chunks(from: u64, to: u64, per: u64) -> impl Iterator<Item = (u64, u64)> {
    let per = per.max(1);
    (from <= to).then_some(()).into_iter().flat_map(move |()| {
        std::iter::successors(Some(from), move |start| start.checked_add(per).filter(|next| *next <= to))
            .map(move |start| (start, to.min(start.saturating_add(per - 1))))
    })
}

fn node<E: std::fmt::Display>(err: E) -> Failure {
    Failure::Node(err.to_string())
}

impl XmrChain for DaemonChain {
    fn status(&self) -> BackendStatus {
        let seen = self.status.lock().unwrap();
        let (error, connecting) = reported(&seen, Instant::now());
        BackendStatus { config: Some(self.config.clone()), network: self.config.network, tip: seen.tip, active: seen.active.clone(), error, connecting }
    }

    fn tip(&self) -> Result<u64, String> {
        let tip = self.call(0, |daemon| async move { daemon.latest_block_number().await.map_err(node) })? as u64;
        self.status.lock().unwrap().tip = Some(tip);
        Ok(tip)
    }

    fn block_hash(&self, number: u64) -> Result<[u8; 32], String> {
        let number = usize::try_from(number).map_err(|err| err.to_string())?;
        self.call(0, |daemon| async move { ProvidesBlockchain::block_hash(&daemon, number).await.map_err(node) })
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<ScannableBlock>, String> {
        let proxied = self.config.nodes.iter().any(|node| self.config.proxied(node));
        let per_request = if proxied { TOR_BLOCKS_PER_REQUEST } else { u64::MAX };
        let mut blocks = Vec::new();
        for (start, end) in chunks(from, to, per_request) {
            let range = usize::try_from(start).map_err(|err| err.to_string())?..=usize::try_from(end).map_err(|err| err.to_string())?;
            blocks.extend(self.call(0, |daemon| {
                let range = range.clone();
                async move { daemon.contiguous_scannable_blocks(range).await.map_err(node) }
            })?);
        }
        Ok(blocks)
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
        let tx = tx.clone();
        self.call(1, move |daemon| {
            let tx = tx.clone();
            async move {
                match daemon.publish_transaction(&tx).await {
                    Ok(()) => Ok(()),
                    Err(PublishTransactionError::TransactionRejected(why)) => {
                        Err(Failure::Final(format!("the Monero network refused the transaction: {why}")))
                    }
                    Err(err) => Err(node(err)),
                }
            }
        })
    }

    fn pool_hashes(&self) -> Result<Vec<[u8; 32]>, String> {
        let text = self.quietly(|daemon| async move {
            daemon.rpc_call("get_transaction_pool_hashes", None, POOL_HASHES_LIMIT).await.map_err(|err| err.to_string())
        })?;
        pool_hashes_from(&text)
    }

    fn pool_transactions(&self, hashes: &[[u8; 32]]) -> Result<Vec<([u8; 32], Transaction<Pruned>)>, String> {
        let hashes = hashes.to_vec();
        self.quietly(|daemon| async move {
            match ProvidesTransactions::pruned_transactions(&daemon, &hashes).await {
                Ok(txs) => Ok(hashes.into_iter().zip(txs).collect()),
                // One left the pool between the two requests: ask one at a time and skip it.
                Err(TransactionsError::TransactionNotFound) => {
                    let mut out = Vec::new();
                    for hash in hashes {
                        match ProvidesTransactions::pruned_transaction(&daemon, hash).await {
                            Ok(tx) => out.push((hash, tx)),
                            Err(TransactionsError::TransactionNotFound) => {}
                            Err(err) => return Err(err.to_string()),
                        }
                    }
                    Ok(out)
                }
                Err(err) => Err(err.to_string()),
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
    fn a_full_failure_is_shown_and_startup_stays_connecting() {
        let start = Instant::now();
        assert_eq!(reported(&Seen::default(), start), (None, true), "nothing answered yet");
        let failing = Seen { error: Some("every Monero node failed".into()), failing_since: Some(start), ..Seen::default() };
        assert_eq!(reported(&failing, start), (Some("every Monero node failed".into()), false));
        let answered = Seen { active: Some("http://a.onion".into()), ..Seen::default() };
        assert_eq!(reported(&answered, start), (None, false));
    }

    #[test]
    fn a_failed_node_is_not_dialed_ahead_of_the_others() {
        assert_eq!(attempt_order(3, 0, |index| index != 0), vec![1, 2]);
        assert_eq!(attempt_order(3, 2, |index| index != 0), vec![2, 1]);
        assert!(attempt_order(3, 0, |_| false).is_empty(), "all three are resting, so nothing is dialed");
        assert_eq!(attempt_order(3, 1, |_| true), vec![1, 2, 0]);
    }

    #[test]
    fn block_ranges_are_split_into_small_requests() {
        assert_eq!(chunks(10, 20, 4).collect::<Vec<_>>(), [(10, 13), (14, 17), (18, 20)]);
        assert_eq!(chunks(10, 10, 4).collect::<Vec<_>>(), [(10, 10)]);
        assert_eq!(chunks(10, 20, u64::MAX).collect::<Vec<_>>(), [(10, 20)]);
        assert_eq!(chunks(u64::MAX - 1, u64::MAX, 4).collect::<Vec<_>>(), [(u64::MAX - 1, u64::MAX)]);
        assert_eq!(chunks(11, 10, 4).count(), 0);
    }

    #[test]
    fn pool_hashes_are_read_and_an_empty_pool_has_none() {
        let hash = "ab".repeat(32);
        assert_eq!(pool_hashes_from(&format!(r#"{{"status":"OK","tx_hashes":["{hash}"]}}"#)).unwrap(), vec![[0xab; 32]]);
        assert!(pool_hashes_from(r#"{"status":"OK"}"#).unwrap().is_empty());
        assert!(pool_hashes_from(r#"{"tx_hashes":["zz"]}"#).is_err());
    }

    #[test]
    fn a_failed_pool_request_rests_no_node_and_sets_no_error() {
        let (first, _) = fake_daemon(Fake::Close);
        let chain = local_chain(&[first]);
        assert!(chain.pool_hashes().is_err());
        assert!(chain.status().error.is_none(), "{:?}", chain.status().error);
        assert!(chain.ready(0), "the next block scan may still use the node");
    }

    fn local_chain(urls: &[String]) -> DaemonChain {
        let nodes = urls.iter().map(|url| DaemonNode { url: url.clone(), kind: NodeKind::Own }).collect();
        DaemonChain::open(DaemonConfig { network: XmrNetwork::Stagenet, nodes, proxy: None }, Duration::from_millis(400)).unwrap()
    }

    #[derive(Clone, Copy)]
    enum Fake { Hang, Close, Answer }

    fn fake_daemon(mode: Fake) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = hits.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut sock) = conn else { break };
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = Vec::new();
                let mut tmp = [0u8; 2048];
                loop {
                    match sock.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                match mode {
                    Fake::Hang => {
                        let _ = sock.set_read_timeout(None);
                        let _ = sock.read(&mut tmp);
                    }
                    Fake::Close => {}
                    Fake::Answer => {
                        let text = String::from_utf8_lossy(&buf);
                        let body = if text.contains("get_height") {
                            r#"{"height":11}"#.to_string()
                        } else {
                            r#"{"jsonrpc":"2.0","id":0,"result":"76ee3cc98646292206cd3e86f74d88b4dcc1d937088645e9b0cbca84b7ce74eb"}"#.to_string()
                        };
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes());
                    }
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    #[test]
    fn a_silent_daemon_does_not_block_the_next_one() {
        let (dead, dead_hits) = fake_daemon(Fake::Hang);
        let (live, live_hits) = fake_daemon(Fake::Answer);
        let chain = local_chain(&[dead, live.clone()]);
        let started = Instant::now();
        let tip = chain.tip().expect("the second daemon answers");
        assert!(started.elapsed() < Duration::from_secs(3), "the silent daemon held the scan for {:?}", started.elapsed());
        assert_eq!(tip, 10, "height is the daemon's block number, one below the raw count");
        assert!(dead_hits.load(Ordering::SeqCst) >= 1, "the silent daemon was not tried");
        assert!(live_hits.load(Ordering::SeqCst) >= 1, "the live daemon was not tried");
        let status = chain.status();
        assert!(status.error.is_none(), "a later success still reports {:?}", status.error);
        assert_eq!(status.tip, Some(10));
        assert_eq!(status.active.as_deref(), Some(live.as_str()));
    }

    #[test]
    fn one_error_is_reported_only_after_every_daemon_fails() {
        let (first, first_hits) = fake_daemon(Fake::Close);
        let (second, second_hits) = fake_daemon(Fake::Close);
        let (third, third_hits) = fake_daemon(Fake::Close);
        let chain = local_chain(&[first.clone(), second.clone(), third.clone()]);
        let started = Instant::now();
        let err = chain.tip().expect_err("no daemon answered");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(err.matches("every Monero node failed").count(), 1, "{err}");
        assert!(err.contains(&first) && err.contains(&second) && err.contains(&third), "{err}");
        assert_eq!(chain.status().error.as_deref(), Some(err.as_str()));
        assert!(chain.status().tip.is_none());
        assert!(first_hits.load(Ordering::SeqCst) >= 1);
        assert!(second_hits.load(Ordering::SeqCst) >= 1);
        assert!(third_hits.load(Ordering::SeqCst) >= 1);
    }

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
