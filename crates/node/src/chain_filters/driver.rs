//! Runs the BIP 157 client and feeds what it hears into the index.
//!
//! One task owns the client. It applies header changes, checks each filter
//! against the followed scripts, fetches matched blocks (the client asks a
//! random connected peer for each one), sends queued rescans, and saves the
//! index. When the client stops (no reachable peers, a crash), the task
//! waits and starts a new client from the saved chain state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bip157::chain::{BlockHeaderChanges, ChainState, IndexedHeader};
use bip157::{Builder, Client, Event, HashCheckpoint, Info, Requester, TrustedPeer, Warning};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver};

use super::index::FilterCheck;
use super::{FilterConfig, Shared};
use crate::buyer_orders::now_secs;

const BLOCK_TIMEOUT: Duration = Duration::from_secs(60);
const RESTART_MIN: Duration = Duration::from_secs(2);
const RESTART_MAX: Duration = Duration::from_secs(60);
/// Save at most this often while filters stream in, and always after a match or a sync.
const SAVE_EVERY: Duration = Duration::from_secs(30);

pub(super) async fn run(shared: Arc<Shared>, config: FilterConfig) {
    let mut pause = RESTART_MIN;
    loop {
        let started = Instant::now();
        let state = chain_state(&shared);
        let (node, client) = builder(&config, state).build();
        let Client { requester, info_rx, warn_rx, event_rx } = client;
        *shared.requester.lock().unwrap_or_else(|err| err.into_inner()) = Some(requester.clone());
        let node = tokio::spawn(node.run());
        let logs = tokio::spawn(log(shared.clone(), info_rx, warn_rx));
        events(&shared, &requester, event_rx).await;
        let _ = requester.shutdown();
        *shared.requester.lock().unwrap_or_else(|err| err.into_inner()) = None;
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
    let peers = config.peers.iter().map(|(host, port)| match host.parse::<std::net::IpAddr>() {
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
    builder
}

fn chain_state(shared: &Shared) -> ChainState {
    let index = shared.index();
    let snapshot = index.snapshot();
    if snapshot.is_empty() {
        return ChainState::Checkpoint(HashCheckpoint::new(index.processed.height, index.processed.hash));
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
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { return };
                shared.contact();
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
            () = shared.wake.notified() => {}
        }
        let rescan = shared.index().take_rescan();
        if let Some(from) = rescan {
            if requester.rescan_from(from).is_err() {
                return;
            }
        }
    }
}

/// Returns `true` when the index should be saved now.
fn header_change(shared: &Shared, change: BlockHeaderChanges) -> bool {
    match change {
        BlockHeaderChanges::Connected(header) => {
            shared.index().connect(header.height, header.header);
            false
        }
        BlockHeaderChanges::Reorganized { accepted, reorganized } => {
            let pairs = |list: Vec<IndexedHeader>| list.into_iter().map(|h| (h.height, h.header)).collect::<Vec<_>>();
            shared.index().reorganize(&pairs(accepted), &pairs(reorganized), now_secs());
            true
        }
        BlockHeaderChanges::ForkAdded(_) => false,
    }
}

/// Fetch a matched block until it arrives. Returns `false` when the client stopped.
async fn fetch_block(shared: &Shared, requester: &Requester, height: u32, hash: bip157::BlockHash) -> bool {
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

async fn log(shared: Arc<Shared>, mut info: Receiver<Info>, mut warnings: Receiver<Warning>) {
    loop {
        tokio::select! {
            message = info.recv() => match message {
                Some(Info::SuccessfulHandshake) => shared.contact(),
                Some(_) => {}
                None => return,
            },
            warning = warnings.recv() => match warning {
                Some(warning @ Warning::TransactionRejected { .. }) => shared.set_error(warning.to_string()),
                Some(_) => {}
                None => return,
            },
        }
    }
}
