//! Runs the BIP 157 client and feeds what it hears into the index.
//!
//! One task owns the client. It applies header changes, checks each filter
//! against the followed scripts, fetches matched blocks (the client asks a
//! random connected peer for each one), sends queued rescans, and saves the
//! index. It also watches transaction gossip for those scripts. Peers announce
//! every unconfirmed transaction; the match stays on this machine, so a public
//! server is never given the wallet's addresses. When the client stops (no
//! reachable peers, a crash), the task waits and starts a new client from the
//! saved chain state.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bip157::chain::{BlockHeaderChanges, ChainState, IndexedHeader};
use bip157::{Builder, Client, Event, HashCheckpoint, Info, Requester, TrustedPeer, Warning};
use bitcoin::{Network, OutPoint, ScriptBuf, Transaction};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use super::index::FilterCheck;
use super::{stale_tip, FilterConfig, Shared, STALE_TIP};
use crate::buyer_orders::now_secs;

const BLOCK_TIMEOUT: Duration = Duration::from_secs(60);
const RESTART_MIN: Duration = Duration::from_secs(2);
const RESTART_MAX: Duration = Duration::from_secs(60);
/// Save at most this often while filters stream in, and always after a match or a sync.
const SAVE_EVERY: Duration = Duration::from_secs(30);
/// A Tor circuit to a new peer is often slower than the client's two-second default.
const TOR_HANDSHAKE: Duration = Duration::from_secs(60);
/// Headers, filters, and blocks over that circuit need longer than five seconds.
const TOR_RESPONSE: Duration = Duration::from_secs(45);
/// How often to notice a caught-up peer whose tip is hours behind the chain.
const STALE_PEER_CHECK: Duration = Duration::from_secs(120);
/// Testnet4 P2P port. Compact-filter seeds are dialed here before a random DNS peer.
const TESTNET4_PORT: u16 = 48333;
/// DNS seeds that advertise `NODE_COMPACT_FILTERS`. Tried before a random peer so a
/// stale or non-filter node cannot be the only connection. The last entry is dialed
/// first (`Vec::pop`).
const TESTNET4_FILTER_SEEDS: &[&str] = &[
    "x49.seed.testnet4.wiz.biz",
    "x49.seed.testnet4.bitcoin.sprovoost.nl",
];

/// What [`builder`] dials, and how long it waits when the connections go through Tor.
pub(super) struct DialPlan {
    pub peers: Vec<(String, u16)>,
    pub handshake_timeout: Option<Duration>,
    pub response_timeout: Option<Duration>,
}

pub(super) fn dial_plan(config: &FilterConfig) -> DialPlan {
    let mut peers = config.peers.clone();
    if config.peers.is_empty() && !config.only_these_peers && config.network == Network::Testnet4 {
        for seed in TESTNET4_FILTER_SEEDS {
            peers.push(((*seed).to_string(), TESTNET4_PORT));
        }
    }
    let (handshake_timeout, response_timeout) = if config.proxy.is_some() {
        (Some(TOR_HANDSHAKE), Some(TOR_RESPONSE))
    } else {
        (None, None)
    };
    DialPlan {
        peers,
        handshake_timeout,
        response_timeout,
    }
}

/// A caught-up scan whose newest header is still the one it already checked, and
/// that header is older than [`STALE_TIP`]. The peer agreed with a tip that has
/// stopped, so a later block is invisible until another peer is dialed.
pub(super) fn stuck_on_stale_tip(
    caught_up: bool,
    header_tip: u32,
    processed: u32,
    header_time: Option<u32>,
    now: u64,
) -> bool {
    caught_up
        && header_tip <= processed
        && header_time.is_some_and(|time| stale_tip(time, now, STALE_TIP))
}

pub(super) async fn run(shared: Arc<Shared>, config: FilterConfig) {
    let mut pause = RESTART_MIN;
    loop {
        let started = Instant::now();
        let state = chain_state(&shared);
        let (node, client) = builder(&config, state).build();
        let Client {
            requester,
            info_rx,
            warn_rx,
            event_rx,
        } = client;
        *shared
            .requester
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(requester.clone());
        let node = tokio::spawn(node.run());
        let logs = tokio::spawn(log(shared.clone(), info_rx, warn_rx));
        events(&shared, &requester, event_rx).await;
        let _ = requester.shutdown();
        *shared
            .requester
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = None;
        shared.peer_up.store(false, Ordering::Relaxed);
        let outcome = match node.await {
            Ok(Ok(())) => "the filter client stopped".to_string(),
            Ok(Err(err)) => format!("the filter client stopped: {err}"),
            Err(err) => format!("the filter client failed: {err}"),
        };
        logs.abort();
        shared.set_error(outcome);
        shared.save();
        if shared.stopping() {
            return;
        }
        if started.elapsed() > RESTART_MAX {
            pause = RESTART_MIN;
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(RESTART_MAX);
    }
}

fn builder(config: &FilterConfig, state: ChainState) -> Builder {
    let plan = dial_plan(config);
    let peers = plan
        .peers
        .iter()
        .map(|(host, port)| match host.parse::<std::net::IpAddr>() {
            Ok(ip) => TrustedPeer::from_socket_addr((ip, *port)),
            Err(_) => TrustedPeer::from_hostname(host.clone(), *port),
        });
    let mut builder = Builder::new(config.network)
        .chain_state(state)
        .required_peers(config.required_peers)
        .fetch_witness_data()
        .add_peers(peers);
    if config.only_these_peers {
        builder = builder.whitelist_only();
    }
    if let Some(proxy) = config.proxy {
        builder = builder.socks5_proxy(proxy);
    }
    if let Some(timeout) = plan.handshake_timeout {
        builder = builder.handshake_timeout(timeout);
    }
    if let Some(timeout) = plan.response_timeout {
        builder = builder.response_timeout(timeout);
    }
    builder
}

fn chain_state(shared: &Shared) -> ChainState {
    let index = shared.index();
    let snapshot = index.snapshot();
    if snapshot.is_empty() {
        return ChainState::Checkpoint(HashCheckpoint::new(
            index.processed.height,
            index.processed.hash,
        ));
    }
    ChainState::Snapshot(
        snapshot
            .into_iter()
            .map(|(height, header)| IndexedHeader { height, header })
            .collect(),
    )
}

async fn events(shared: &Shared, requester: &Requester, mut events: UnboundedReceiver<Event>) {
    let mut saved = Instant::now();
    let mut gossip: Option<Receiver<Transaction>> = None;
    let mut followed_scripts = HashSet::new();
    let mut followed_inputs = HashSet::new();
    let mut refresh_gossip = true;
    let mut stale_peer = tokio::time::interval(STALE_PEER_CHECK);
    stale_peer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    stale_peer.tick().await;
    loop {
        if refresh_gossip {
            refresh_gossip = false;
            if let Err(err) = subscribe_gossip(
                shared,
                requester,
                &mut gossip,
                &mut followed_scripts,
                &mut followed_inputs,
            )
            .await
            {
                eprintln!("ob-node: mempool watch: {err}");
            }
        }
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { return };
                shared.contact();
                shared.peer_up.store(true, Ordering::Relaxed);
                let save_now = match event {
                    Event::ChainUpdate(change) => header_change(shared, change),
                    Event::IndexedFilter(filter) => {
                        let height = filter.height();
                        let hash = filter.block_hash();
                        let check = shared.index().check_filter(height, hash, &filter.into_contents());
                        let matched = match check {
                            FilterCheck::Fetch => {
                                if !fetch_block(shared, requester, height, hash).await {
                                    return;
                                }
                                true
                            }
                            FilterCheck::Skip => false,
                            FilterCheck::Conflict => {
                                shared.set_error(format!("a peer served a different filter for block {height}; the first one stands"));
                                false
                            }
                        };
                        shared.index().filter_done(height, hash);
                        matched
                    }
                    Event::FiltersSynced(_) => {
                        let mut index = shared.index();
                        index.synced();
                        index.expire_mempool(now_secs());
                        true
                    }
                };
                if save_now || saved.elapsed() > SAVE_EVERY {
                    shared.save();
                    saved = Instant::now();
                }
            }
            tx = next_gossip(&mut gossip) => {
                match tx {
                    Some(tx) => {
                        if shared.index().note_unconfirmed(tx, now_secs()) {
                            shared.save();
                            refresh_gossip = true;
                        }
                    }
                    None => {
                        gossip = None;
                        followed_scripts.clear();
                        followed_inputs.clear();
                        let _ = requester.unsubscribe_from_gossip();
                        refresh_gossip = true;
                    }
                }
            }
            () = shared.wake.notified() => refresh_gossip = true,
            _ = stale_peer.tick() => {
                if peer_stopped(&shared) {
                    eprintln!("ob-node: filter tip is hours old and no newer header arrived; dropping this peer to dial another");
                    return;
                }
            }
        }
        let rescan = shared.index().take_rescan();
        if let Some(from) = rescan {
            if requester.rescan_from(from).is_err() {
                return;
            }
        }
    }
}

/// Ask the connected peers for every unconfirmed transaction, and keep the
/// ones that pay a script this node already follows. The script list is not
/// sent: peers announce the whole mempool with `inv`.
async fn subscribe_gossip(
    shared: &Shared,
    requester: &Requester,
    gossip: &mut Option<Receiver<Transaction>>,
    followed_scripts: &mut HashSet<ScriptBuf>,
    followed_inputs: &mut HashSet<OutPoint>,
) -> Result<(), String> {
    let (scripts, inputs) = {
        let index = shared.index();
        let scripts: Vec<ScriptBuf> = index
            .scripts
            .keys()
            .filter(|script| !followed_scripts.contains(*script))
            .cloned()
            .collect();
        let inputs: Vec<OutPoint> = index
            .mempool
            .values()
            .flat_map(|(tx, _)| tx.input.iter().map(|input| input.previous_output))
            .filter(|outpoint| !followed_inputs.contains(outpoint))
            .collect();
        (scripts, inputs)
    };
    if scripts.is_empty() && inputs.is_empty() {
        return Ok(());
    }
    let request =
        bip157::GossipMonitorRequestBuilder::from_expected_output_scripts(scripts.iter().cloned())
            .add_outpoints(inputs.iter().copied())
            .build();
    let incoming = requester
        .subscribe_to_gossip(request)
        .await
        .map_err(|err| err.to_string())?;
    if incoming.is_none() && gossip.is_none() {
        return Err("the mempool watch did not start".into());
    }
    if let Some(incoming) = incoming {
        *gossip = Some(incoming);
    }
    followed_scripts.extend(scripts);
    followed_inputs.extend(inputs);
    Ok(())
}

async fn next_gossip(gossip: &mut Option<Receiver<Transaction>>) -> Option<Transaction> {
    match gossip.as_mut() {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Returns `true` when the index should be saved now.
fn header_change(shared: &Shared, change: BlockHeaderChanges) -> bool {
    match change {
        BlockHeaderChanges::Connected(header) => {
            shared.index().connect(header.height, header.header);
            false
        }
        BlockHeaderChanges::Reorganized {
            accepted,
            reorganized,
        } => {
            let pairs = |list: Vec<IndexedHeader>| {
                list.into_iter()
                    .map(|h| (h.height, h.header))
                    .collect::<Vec<_>>()
            };
            shared
                .index()
                .reorganize(&pairs(accepted), &pairs(reorganized), now_secs());
            true
        }
        BlockHeaderChanges::ForkAdded(_) => false,
    }
}

/// Fetch a matched block until it arrives. Returns `false` when the client stopped.
async fn fetch_block(
    shared: &Shared,
    requester: &Requester,
    height: u32,
    hash: bip157::BlockHash,
) -> bool {
    loop {
        match tokio::time::timeout(BLOCK_TIMEOUT, requester.get_block(hash)).await {
            Ok(Ok(block)) => {
                shared.index().apply_block(height, &block.block);
                return true;
            }
            Ok(Err(err)) => shared.set_error(format!("block {height}: {err}")),
            Err(_) => shared.set_error(format!("block {height} did not arrive")),
        }
        if !requester.is_running() {
            return false;
        }
    }
}

fn peer_stopped(shared: &Shared) -> bool {
    let index = shared.index();
    let header_tip = index
        .headers
        .keys()
        .next_back()
        .copied()
        .unwrap_or(index.processed.height);
    let header_time = index
        .headers
        .get(&index.processed.height)
        .map(|header| header.time);
    stuck_on_stale_tip(
        index.caught_up,
        header_tip,
        index.processed.height,
        header_time,
        now_secs(),
    )
}

async fn log(shared: Arc<Shared>, mut info: Receiver<Info>, mut warnings: Receiver<Warning>) {
    loop {
        tokio::select! {
            message = info.recv() => match message {
                Some(Info::SuccessfulHandshake) => {
                    shared.contact();
                    shared.peer_up.store(true, Ordering::Relaxed);
                }
                Some(_) => {}
                None => return,
            },
            warning = warnings.recv() => match warning {
                Some(warning @ Warning::TransactionRejected { .. }) => shared.set_error(warning.to_string()),
                // NeedConnections { connected: 0 } is sent every 10ms while a peer is still
                // dialing, so leftover warnings after the handshake would clear peer_up.
                Some(Warning::NeedConnections { .. }) => {}
                Some(warning) => eprintln!("ob-node: filter peer: {warning}"),
                None => return,
            },
        }
    }
}
