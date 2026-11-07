use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::{Duration, Instant};

use bdk_wallet::chain::BlockId;
use bdk_wallet::{KeychainKind, Wallet};
use bitcoin::hashes::Hash;
use bitcoin::{Network, OutPoint, ScriptBuf};

use super::index::{FilterCheck, FilterIndex, NEW_SCRIPT_LOOKBACK};
use super::mock_peer::{pay, spend, MockPeer, SocksRecorder, TestChain};
use super::store::{decode, encode, ChainStore};
use super::{stale_tip, FilterChain, FilterConfig, STALE_TIP};
use crate::chain::{ChainSource, WalletRequest};
use crate::harness::TempDir;
use crate::network::NetworkMode;
use crate::vault::Vault;
use crate::ServeOptions;

const EXTERNAL: &str = "tr([7d94197e/86'/1'/0']tpubDCyQVJj8KzjiQsFjmb3KwECVXPvMwvAxxZGCP9XmWSopmjW3bCV3wD7TgxrUhiGSueDS1MU5X1Vb1YjYcp8jitXc5fXfdC1z68hDDEyKRNr/0/*)";
const INTERNAL: &str = "tr([7d94197e/86'/1'/0']tpubDCyQVJj8KzjiQsFjmb3KwECVXPvMwvAxxZGCP9XmWSopmjW3bCV3wD7TgxrUhiGSueDS1MU5X1Vb1YjYcp8jitXc5fXfdC1z68hDDEyKRNr/1/*)";

fn genesis(chain: &TestChain) -> BlockId {
    BlockId { height: 0, hash: chain.hash(0) }
}

fn script(tag: u8) -> ScriptBuf {
    ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([tag; 32]))
}

#[test]
fn a_tip_more_than_two_hours_behind_the_clock_is_stale() {
    let now = 1_800_000_000u64;
    let fresh = u32::try_from(now - 30 * 60).unwrap();
    let old = u32::try_from(now - STALE_TIP.as_secs() - 1).unwrap();
    assert!(!stale_tip(fresh, now, STALE_TIP));
    assert!(stale_tip(old, now, STALE_TIP));
    assert!(!stale_tip(u32::try_from(now).unwrap(), now, STALE_TIP));
}

/// Feed every block above `from` to the index the way the driver does.
fn scan(index: &mut FilterIndex, chain: &TestChain, from: u32) -> Vec<u32> {
    let mut fetched = Vec::new();
    for height in from + 1..=chain.tip() {
        let block = &chain.blocks[height as usize];
        index.connect(height, block.header);
        if index.check_filter(height, chain.hash(height), &chain.filter(height)) == FilterCheck::Fetch {
            index.apply_block(height, block);
            fetched.push(height);
        }
        index.filter_done(height, chain.hash(height));
    }
    index.synced();
    fetched
}

fn wallet() -> Wallet {
    Wallet::create(EXTERNAL, INTERNAL).network(Network::Regtest).create_wallet_no_persist().unwrap()
}

fn wallet_spks(wallet: &Wallet, count: u32) -> Vec<(Option<(KeychainKind, u32)>, ScriptBuf)> {
    [KeychainKind::External, KeychainKind::Internal]
        .into_iter()
        .flat_map(|keychain| (0..count).map(move |index| (keychain, index)))
        .map(|(keychain, index)| (Some((keychain, index)), wallet.peek_address(keychain, index).script_pubkey()))
        .collect()
}

#[test]
fn only_blocks_that_touch_a_followed_script_are_fetched() {
    let mut chain = TestChain::new();
    for _ in 0..3 {
        chain.mine(vec![]);
    }
    chain.mine(vec![pay(&script(1), 7_000, 1), pay(&script(9), 1, 2)]);
    chain.mine(vec![pay(&script(9), 5, 3)]);
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    assert!(!index.watch([script(1)]), "nothing scanned yet, so no rescan is needed");
    assert!(index.tip().is_err(), "no tip before the first sync");

    assert_eq!(scan(&mut index, &chain, 0), [4]);
    assert_eq!(index.tip().unwrap(), 5);
    let txs = index.address_txs(&script(1));
    assert_eq!(txs.len(), 1);
    assert_eq!((txs[0].height, txs[0].value()), (Some(4), 7_000));
    let stored: Vec<_> = index.blocks[&4].txs.iter().map(|tx| tx.output[0].script_pubkey.clone()).collect();
    assert_eq!(stored, [script(1)], "the unrelated payment in the same block is not kept");
    assert_eq!(index.commitments.len(), 5);
}

#[test]
fn wallet_updates_follow_payments_spends_and_a_reorg() {
    let mut wallet = wallet();
    let spks = wallet_spks(&wallet, 5);
    let receive = spks[1].1.clone();
    let mut chain = TestChain::new();
    chain.mine(vec![]);
    chain.mine(vec![pay(&receive, 40_000, 1)]);
    chain.mine(vec![]);
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    index.watch(spks.iter().map(|(_, script)| script.clone()));
    scan(&mut index, &chain, 0);
    wallet.apply_update(index.wallet_update(&spks)).unwrap();
    assert_eq!(wallet.balance().confirmed.to_sat(), 40_000);
    assert_eq!(wallet.latest_checkpoint().height(), 3);
    assert_eq!(wallet.derivation_index(KeychainKind::External), Some(1), "the used index is revealed");

    // Spending the coin to someone else: the filter matches on the spent script.
    let paid = OutPoint { txid: chain.blocks[2].txdata[1].compute_txid(), vout: 0 };
    chain.mine(vec![spend(paid, &script(7), 39_000)]);
    assert_eq!(scan(&mut index, &chain, 3), [4]);
    wallet.apply_update(index.wallet_update(&spks)).unwrap();
    assert_eq!(wallet.balance().total().to_sat(), 0);

    // Blocks 2..4 are replaced by a longer chain that holds neither transaction yet.
    let removed: Vec<_> = (2..=4).map(|height| (height, chain.blocks[height as usize].header)).collect();
    let payment = chain.blocks[2].txdata[1].clone();
    chain.truncate(1);
    for _ in 0..4 {
        chain.mine(vec![]);
    }
    let accepted: Vec<_> = (2..=5).map(|height| (height, chain.blocks[height as usize].header)).collect();
    index.reorganize(&accepted, &removed, 1_000);
    assert_eq!(index.processed.height, 1);
    assert!(index.blocks.is_empty());
    assert!(scan(&mut index, &chain, 1).is_empty());
    assert_eq!(index.tip().unwrap(), 5);
    wallet.apply_update(index.wallet_update(&spks)).unwrap();
    assert_eq!(wallet.latest_checkpoint().block_id(), BlockId { height: 5, hash: chain.hash(5) });
    assert_eq!(wallet.balance().confirmed.to_sat(), 0, "nothing is confirmed on the new chain");
    let txs = index.address_txs(&receive);
    assert_eq!((txs.len(), txs[0].height), (1, None), "the payment is unconfirmed again");

    // The payment confirms again on the new chain.
    chain.mine(vec![payment]);
    assert_eq!(scan(&mut index, &chain, 5), [6]);
    assert_eq!(index.address_txs(&receive)[0].height, Some(6));
    assert!(!index.mempool.contains_key(&chain.blocks[6].txdata[1].compute_txid()));
    wallet.apply_update(index.wallet_update(&spks)).unwrap();
    assert_eq!(wallet.balance().total().to_sat(), 0, "the confirmed coin is still spent by the pending spend");
    assert_eq!(index.mempool.len(), 1);
}

#[test]
fn a_new_script_is_rescanned_before_it_is_reported() {
    let mut chain = TestChain::new();
    for height in 1..=10 {
        chain.mine(if height == 6 { vec![pay(&script(2), 3_000, 4)] } else { vec![] });
    }
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    index.watch([script(1)]);
    assert!(scan(&mut index, &chain, 0).is_empty());

    assert!(index.watch([script(2)]));
    assert!(!index.is_ready(&script(2)), "an address added after the scan is not reported yet");
    assert!(index.is_ready(&script(1)));
    let from = index.take_rescan().unwrap();
    assert_eq!(from, 10u32.saturating_sub(NEW_SCRIPT_LOOKBACK));
    assert!(index.take_rescan().is_none(), "one rescan at a time");
    assert_eq!(scan(&mut index, &chain, from), [6]);
    assert!(index.is_ready(&script(2)));
    assert_eq!(index.address_txs(&script(2))[0].height, Some(6));
    assert_eq!(index.tip().unwrap(), 10, "a rescan does not move the tip backward");
}

#[test]
fn an_escrow_script_is_followed_from_the_height_asked_not_the_lookback() {
    let mut chain = TestChain::new();
    let deep = NEW_SCRIPT_LOOKBACK + 20;
    for height in 1..=deep {
        chain.mine(if height == 3 { vec![pay(&script(7), 9_000, 9)] } else { vec![] });
    }
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    index.watch([script(1)]);
    assert!(scan(&mut index, &chain, 0).is_empty());

    let mut lookback = index.clone();
    assert!(lookback.watch([script(7)]));
    let from = lookback.take_rescan().unwrap();
    assert!(from > 3, "the wallet lookback starts at {from}, after the funding");

    assert!(index.watch_from([script(7)], 2));
    assert_eq!(index.take_rescan(), Some(2));
    assert_eq!(scan(&mut index, &chain, 2), [3]);
    assert!(index.is_ready(&script(7)));
    assert_eq!(index.address_txs(&script(7))[0].height, Some(3));
    assert!(!index.watch_from([script(7)], 1), "a followed script is not rescanned again");
}

#[test]
fn a_peer_that_changes_a_checked_filter_is_refused() {
    let mut chain = TestChain::new();
    chain.mine(vec![pay(&script(1), 1_000, 1)]);
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    index.watch([script(1)]);
    assert_eq!(index.check_filter(1, chain.hash(1), &chain.filter(1)), FilterCheck::Fetch);
    let mut empty = TestChain::new();
    empty.mine(vec![]);
    assert_eq!(index.check_filter(1, chain.hash(1), &empty.filter(1)), FilterCheck::Conflict);
}

#[test]
fn the_index_is_sealed_on_disk_and_survives_a_restart() {
    let mut chain = TestChain::new();
    chain.mine(vec![pay(&script(1), 1_000, 1)]);
    chain.mine(vec![]);
    let mut index = FilterIndex::new(Network::Regtest, genesis(&chain));
    index.watch([script(1)]);
    scan(&mut index, &chain, 0);
    index.watch([script(3)]);

    let dir = TempDir::new();
    let store = store(&dir.0);
    store.save(&index).unwrap();
    let on_disk = std::fs::read(dir.0.join("filters/state.obv")).unwrap();
    assert!(!String::from_utf8_lossy(&on_disk).contains("scripts"), "state is sealed");
    let hex: String = script(1).as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(!String::from_utf8_lossy(&on_disk).contains(&hex), "followed scripts are not readable on disk");

    let back = store.load(Network::Regtest).unwrap().unwrap();
    assert_eq!(back.processed, index.processed);
    assert_eq!(back.blocks, index.blocks);
    assert_eq!(back.commitments, index.commitments);
    assert_eq!(back.headers, index.headers);
    assert!(back.is_ready(&script(1)));
    assert!(!back.is_ready(&script(3)), "an unfinished rescan is still owed after a restart");
    let (state, commitments) = encode(&back);
    assert!(decode(Network::Bitcoin, &state, &commitments).is_err(), "another network's index is refused");
}

/// A filter store in `dir`, sealed with a file vault.
fn store(dir: &Path) -> ChainStore {
    std::fs::create_dir_all(dir).unwrap();
    ChainStore { dir: dir.join("filters"), vault: Vault::open(dir, &crate::VaultMode::File).unwrap(), wallet_tip: None }
}

fn open_chain(config: FilterConfig, dir: &TempDir) -> FilterChain {
    let chain = FilterChain::new(config).unwrap();
    chain.open(store(&dir.0)).unwrap();
    chain
}

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn own_node(peer: &MockPeer) -> FilterConfig {
    FilterConfig::own_node(Network::Regtest, vec![(peer.addr.ip().to_string(), peer.addr.port())])
}

#[test]
fn the_filter_client_syncs_from_a_peer_finds_payments_and_broadcasts() {
    let wallet_template = wallet();
    let spks = wallet_spks(&wallet_template, 3);
    let receive = spks[0].1.clone();
    let order = script(5);
    let mut chain = TestChain::new();
    for height in 1..=8 {
        chain.mine(match height {
            3 => vec![pay(&receive, 21_000, 1)],
            6 => vec![pay(&order, 9_000, 2)],
            _ => vec![],
        });
    }
    let peer = MockPeer::start(chain.clone());
    let dir = TempDir::new();
    let filters = open_chain(own_node(&peer), &dir);
    let order_address = bitcoin::Address::from_script(&order, Network::Regtest).unwrap();

    let txs = wait_for("the order payment", || filters.address_txs(&order_address).ok().filter(|txs| !txs.is_empty()));
    assert_eq!((txs[0].height, txs[0].value()), (Some(6), 9_000));
    assert_eq!(filters.tip_height().unwrap(), 8);

    let mut wallet = wallet();
    let update = wait_for("a wallet update", || {
        filters.wallet_update(WalletRequest::Full(wallet.start_full_scan().build())).ok()
    });
    wallet.apply_update(update).unwrap();
    assert_eq!(wallet.balance().confirmed.to_sat(), 21_000);
    let served = peer.blocks_served();
    assert!(served.contains(&chain.hash(3)) && served.contains(&chain.hash(6)));
    assert!(!served.contains(&chain.hash(4)), "blocks without a match are never fetched");

    let coin = OutPoint { txid: chain.blocks[3].txdata[1].compute_txid(), vout: 0 };
    let outgoing = spend(coin, &script(6), 20_000);
    filters.broadcast(&outgoing).unwrap();
    wait_for("the broadcast at the peer", || peer.received().contains(&outgoing).then_some(()));
    assert_eq!(filters.address_txs(&order_address).unwrap().len(), 1);
    let status = filters.status();
    assert_eq!((status.backend, status.privacy, status.syncing), ("own-node", "own-node", false));
    assert_eq!(status.scanned_height, Some(8));
}

#[test]
fn a_reorg_at_the_peer_moves_a_payment_back_to_unconfirmed() {
    let order = script(8);
    let mut chain = TestChain::new();
    for height in 1..=5 {
        chain.mine(if height == 4 { vec![pay(&order, 4_000, 3)] } else { vec![] });
    }
    let payment = chain.blocks[4].txdata[1].clone();
    let peer = MockPeer::start(chain);
    let dir = TempDir::new();
    let filters = open_chain(own_node(&peer), &dir);
    let address = bitcoin::Address::from_script(&order, Network::Regtest).unwrap();
    wait_for("the payment at height 4", || {
        filters.address_txs(&address).ok().filter(|txs| txs.first().and_then(|tx| tx.height) == Some(4))
    });

    peer.update(3, |chain| {
        chain.truncate(3);
        for _ in 0..3 {
            chain.mine(vec![]);
        }
    });
    let txs = wait_for("the reorg", || {
        (filters.tip_height().ok() == Some(6)).then(|| filters.address_txs(&address).ok()).flatten()
    });
    assert_eq!((txs.len(), txs[0].height), (1, None), "{txs:?}");

    peer.update(6, |chain| {
        chain.mine(vec![payment]);
    });
    wait_for("the payment at height 7", || {
        filters.address_txs(&address).ok().filter(|txs| txs.first().and_then(|tx| tx.height) == Some(7))
    });

    // A restart picks up from the saved index without scanning from genesis again.
    drop(filters);
    let again = open_chain(own_node(&peer), &dir);
    let txs = wait_for("the reopened index", || again.address_txs(&address).ok());
    assert_eq!(txs[0].height, Some(7));
    assert_eq!(again.tip_height().unwrap(), 7);
}

#[test]
fn own_node_mode_never_contacts_anyone_else() {
    let mut chain = TestChain::new();
    chain.mine(vec![pay(&script(4), 2_000, 5)]);
    let peer = MockPeer::start(chain);
    let socks = SocksRecorder::start(peer.addr);
    let peer_flag = format!("{}:{}", peer.addr.ip(), peer.addr.port());
    for network in ["testnet4", "mainnet"] {
        let options = ServeOptions::parse(&["--network".into(), network.into(), "--peer".into(), peer_flag.clone()]).unwrap();
        let plan = options.chain_plan();
        assert!(plan.esplora_servers().is_empty(), "{network}: no Esplora server, not even as a fallback");
        assert!(!options.uses_public_server());
        let crate::ChainPlan::Filters { config, .. } = plan else { panic!("own node uses filters") };
        assert!(config.only_these_peers && config.peers == [(peer.addr.ip().to_string(), peer.addr.port())]);
        let built = crate::ChainPlan::Filters { config, fallback: Vec::new() }
            .build(NetworkMode::Testnet4, &crate::Egress::default())
            .unwrap();
        assert_eq!(built.status().backend, "own-node");
    }

    let egress = crate::Egress { proxy: Some(format!("socks5h://{}", socks.addr)), live_rates: false };
    let filters = crate::ChainPlan::Filters { config: own_node(&peer), fallback: Vec::new() }
        .build(NetworkMode::Testnet4, &egress)
        .unwrap();
    let dir = TempDir::new();
    filters.open(store(&dir.0)).unwrap();
    let address = bitcoin::Address::from_script(&script(4), Network::Regtest).unwrap();
    wait_for("the payment through the proxy", || filters.address_txs(&address).ok().filter(|txs| !txs.is_empty()));
    std::thread::sleep(Duration::from_millis(500));
    let seen = socks.seen.lock().unwrap().clone();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|dest| *dest == peer_flag), "only the user's node was dialed: {seen:?}");
    assert!(peer.connections() >= 1);
}

#[test]
fn a_public_filter_peer_is_not_dialed_without_a_proxy() {
    let filters = FilterChain::new(FilterConfig::public(Network::Testnet4)).unwrap();
    let dir = TempDir::new();
    filters.open(store(&dir.0)).unwrap();
    let started = Instant::now();
    let err = filters.broadcast(&pay(&script(1), 1_000, 1)).unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(500), "a public peer was dialed");
    assert!(err.contains("without --proxy"), "{err}");
    let status = filters.status();
    assert!(status.reveals.contains("not contacted"), "{}", status.reveals);
    assert!(status.peers.is_none(), "the filter client started");
}

/// Sync testnet4 compact filters from public P2P peers and print how long it
/// took. Run with
/// `cargo test -p ob-node live_testnet4_filters -- --ignored --nocapture`.
/// Set `OB_LIVE_ADDRESS` to also list one address's transactions, and
/// `OB_LIVE_FILTER_DIR` to keep the index between runs.
#[test]
#[ignore]
fn live_testnet4_filters() {
    let Some(proxy) = std::env::var("OB_PROXY").ok() else {
        eprintln!("skipped: public filter peers are not dialed without a proxy (set OB_PROXY=socks5h://127.0.0.1:9050)");
        return;
    };
    let authority = proxy.trim_start_matches("socks5h://").trim_start_matches("socks5://");
    let Some(proxy_addr) = authority.to_socket_addrs().ok().and_then(|mut addrs| addrs.next()) else {
        eprintln!("skipped: OB_PROXY must be socks5h://HOST:PORT");
        return;
    };
    let temp = TempDir::new();
    let dir = std::env::var("OB_LIVE_FILTER_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| temp.0.clone());
    let started = Instant::now();
    let mut config = FilterConfig::public(Network::Testnet4);
    config.proxy = Some(proxy_addr);
    let filters = FilterChain::new(config).unwrap();
    filters
        .open(store(&dir))
        .unwrap();
    let address = std::env::var("OB_LIVE_ADDRESS").ok().map(|text| {
        text.parse::<bitcoin::Address<_>>().unwrap().require_network(Network::Testnet4).unwrap()
    });
    let tip = loop {
        assert!(started.elapsed() < Duration::from_secs(1800), "no sync after 30 minutes: {:?}", filters.status());
        match filters.tip_height() {
            Ok(tip) => break tip,
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    };
    println!("testnet4 filters synced to {tip} in {:?}: {}", started.elapsed(), filters.status().to_json());
    if let Some(address) = address {
        let txs = loop {
            match filters.address_txs(&address) {
                Ok(txs) => break txs,
                Err(_) => std::thread::sleep(Duration::from_secs(2)),
            }
        };
        println!("{address}: {} transactions after the rescan, {:?} total", txs.len(), started.elapsed());
    }
}

#[test]
fn empty_blocks_do_not_drag_a_live_feerate_down_to_the_floor() {
    use crate::pay_order::MIN_FEE_RATE;

    // Newest first, the order the estimator walks back from the tip.
    // Empty blocks average 0. The blocks that included transactions paid
    // 2 sat/vB. Taking the median of the raw window, zeroes included, is 0,
    // and the payment floor then pins the spend at 1 sat/vB.
    let behind_a_run_of_empty = [0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 2.0, 2.0];
    assert!(super::FEE_LOOKBACK as usize >= behind_a_run_of_empty.len());
    let estimate = super::fee_rate_from_blocks(&behind_a_run_of_empty, super::FEE_BLOCKS as usize).unwrap();
    assert_eq!(estimate.max(MIN_FEE_RATE), 2.0);

    // The same rule on the short window the estimator used to median directly.
    let majority_empty = [0.0, 0.0, 2.0];
    assert_eq!(
        super::fee_rate_from_blocks(&majority_empty, super::FEE_BLOCKS as usize).unwrap().max(MIN_FEE_RATE),
        2.0
    );

    // No empty blocks: still the median of the paid rates.
    assert_eq!(super::fee_rate_from_blocks(&[1.0, 5.0, 3.0], 3), Some(3.0));
    // Nobody paid a fee. The floor is the whole estimate, not a rate miners took.
    assert_eq!(super::fee_rate_from_blocks(&[0.0, 0.0, 0.0], 3), Some(0.0));
    assert_eq!(0.0_f64.max(MIN_FEE_RATE), MIN_FEE_RATE);
}

#[test]
fn a_scan_that_stops_moving_while_behind_counts_as_unreachable() {
    let after = Duration::from_secs(120);
    let start = Instant::now();
    let mut progress = None;
    assert!(!super::stalled(&mut progress, 100, false, start, after));
    assert!(!super::stalled(&mut progress, 100, false, start + Duration::from_secs(60), after));
    assert!(super::stalled(&mut progress, 100, false, start + Duration::from_secs(121), after), "stuck at 100");
    assert!(!super::stalled(&mut progress, 101, false, start + Duration::from_secs(122), after), "it moved");
    assert!(!super::stalled(&mut progress, 101, true, start + Duration::from_secs(900), after), "caught up and waiting for a block");
}
