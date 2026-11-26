//! Which chain backend the node runs, chosen from `ob-node serve` flags.
//!
//! | Flags | Backend | Who learns the user's addresses |
//! |---|---|---|
//! | none | compact filters from P2P peers | no one |
//! | `--peer HOST:PORT` | compact filters from the user's own node only | that node |
//! | `--esplora URL` (LAN, loopback, onion) | the user's Esplora server | that server |
//! | `--esplora URL` (anything else) | that public Esplora server, for the tip, fees, and broadcasts, and only through `--proxy` | no one (addresses are not sent; without a proxy the server is not contacted) |
//!
//! With no flags, `--public-fallback` (on by default on testnet4 only) lets
//! the node ask the public Esplora servers for the tip, fees, and broadcasts
//! while no filter peer answers. Address lookups and wallet scans stay on
//! the filters. `--peer` and `--esplora` never fall back to anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bdk_wallet::Update;
use bitcoin::{Address, OutPoint, Transaction, TxOut};

use crate::chain::{AddressTx, ChainSource, ChainStatus, EsploraChain, WalletRequest};
use crate::chain_filters::{ChainStore, FilterChain, FilterConfig};
use crate::network::NetworkMode;
use crate::Egress;

/// How long the filter client must go without any peer before the fallback answers.
const FALLBACK_AFTER: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainPlan {
    /// Compact block filters. `fallback` lists public Esplora servers, or is empty.
    Filters { config: FilterConfig, fallback: Vec<String> },
    /// Exactly these Esplora servers.
    Esplora { urls: Vec<String> },
}

impl ChainPlan {
    pub(crate) fn choose(
        network: NetworkMode,
        peers: &[(String, u16)],
        esplora: &[String],
        public_fallback: bool,
    ) -> Result<Self, String> {
        match (peers.is_empty(), esplora.is_empty()) {
            (false, false) => Err("use --peer or --esplora, not both".into()),
            (false, true) if public_fallback => Err("--peer is your own node; it never falls back to public servers, so drop --public-fallback".into()),
            (false, true) => Ok(ChainPlan::Filters {
                config: FilterConfig::own_node(network.bitcoin(), peers.to_vec()),
                fallback: Vec::new(),
            }),
            (true, false) => Ok(ChainPlan::Esplora { urls: esplora.to_vec() }),
            (true, true) => Ok(ChainPlan::Filters {
                config: FilterConfig::public(network.bitcoin()),
                fallback: if public_fallback {
                    network.default_esplora().iter().map(|url| url.to_string()).collect()
                } else {
                    Vec::new()
                },
            }),
        }
    }

    /// Dial mempool.space at its onion. The path stays. Blockstream stays on clearnet.
    pub(crate) fn through_proxy(mut self) -> Self {
        let rewrite = |urls: &mut [String]| {
            for url in urls {
                *url = crate::network::esplora_base(url, true);
            }
        };
        match &mut self {
            ChainPlan::Filters { fallback, .. } => rewrite(fallback),
            ChainPlan::Esplora { urls } => rewrite(urls),
        }
        self
    }

    /// Every Esplora server this plan may contact. A public one is not asked for addresses.
    pub fn esplora_servers(&self) -> &[String] {
        match self {
            ChainPlan::Filters { fallback, .. } => fallback,
            ChainPlan::Esplora { urls } => urls,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            ChainPlan::Filters { config, .. } if config.only_these_peers => {
                let peers: Vec<String> = config.peers.iter().map(|(host, port)| format!("{host}:{port}")).collect();
                format!("compact block filters from your own node only ({})", peers.join(", "))
            }
            ChainPlan::Filters { fallback, .. } if fallback.is_empty() => {
                "compact block filters from P2P peers; no public server fallback".into()
            }
            ChainPlan::Filters { fallback, .. } => format!(
                "compact block filters from P2P peers; public Esplora ({}) for the tip and the fees while no peer answers, and for any transaction no peer takes, only through --proxy",
                fallback.join(", ")
            ),
            ChainPlan::Esplora { urls } => format!("Esplora at {}", urls.join(", ")),
        }
    }

    /// With a proxy in `egress`, filter peers, broadcasts and Esplora queries all go through it.
    pub fn build(&self, network: NetworkMode, egress: &Egress) -> Result<Box<dyn ChainSource>, String> {
        let filters = |config: &FilterConfig| {
            FilterChain::new(FilterConfig {
                proxy: egress.proxy.as_deref().map(crate::egress::proxy_addr).transpose()?,
                ..config.clone()
            })
        };
        Ok(match self {
            ChainPlan::Filters { config, fallback } if fallback.is_empty() => Box::new(filters(config)?),
            ChainPlan::Filters { config, fallback } => Box::new(FallbackChain {
                primary: filters(config)?,
                fallback: EsploraChain::with_egress(fallback, network.bitcoin(), egress)?,
                active: AtomicBool::new(false),
                public_broadcast: AtomicBool::new(false),
            }),
            ChainPlan::Esplora { urls } => Box::new(EsploraChain::with_egress(urls, network.bitcoin(), egress)?),
        })
    }
}

/// Parse one `--peer` value: `HOST:PORT`, `[IPv6]:PORT`, or `HOST` on the network's P2P port.
pub(crate) fn parse_peer(text: &str, network: NetworkMode) -> Result<(String, u16), String> {
    let text = text.trim();
    let refused = || format!("--peer must be HOST:PORT, not {text:?}");
    let default_port = match network {
        NetworkMode::Testnet4 => 48333,
        NetworkMode::Mainnet => 8333,
    };
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(refused)?;
        match after.strip_prefix(':') {
            Some(port) => (host, Some(port)),
            None if after.is_empty() => (host, None),
            None => return Err(refused()),
        }
    } else {
        match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return Err(refused()),
            None => (text, None),
        }
    };
    if host.is_empty() || host.contains(['/', '@', ' ']) {
        return Err(refused());
    }
    let port = match port {
        Some(port) => port.parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(refused)?,
        None => default_port,
    };
    Ok((host.to_string(), port))
}

/// Compact filters first. Public Esplora answers the tip, fees, and broadcasts
/// while no peer answers or the scan is stuck. It is never asked for an address.
struct FallbackChain {
    primary: FilterChain,
    fallback: EsploraChain,
    active: AtomicBool,
    /// Set once a transaction has been handed to a public server, so status()
    /// can say so after the fact.
    public_broadcast: AtomicBool,
}

impl FallbackChain {
    fn use_fallback(&self) -> bool {
        let unreachable = self.primary.unreachable_for(FALLBACK_AFTER);
        if self.active.swap(unreachable, Ordering::Relaxed) != unreachable {
            if unreachable {
                eprintln!("ob-node: WARNING: no filter peer answered or the filter scan stopped moving; public Esplora is asked for the tip and the fees, and for any transaction no peer takes, only when --proxy is set. Addresses stay on the filters, so a sale address is not seen until a peer answers");
            } else {
                eprintln!("ob-node: filter peers answer again; the public Esplora servers are no longer asked");
            }
        }
        unreachable
    }

    fn pick(&self) -> &dyn ChainSource {
        if self.use_fallback() {
            &self.fallback
        } else {
            &self.primary
        }
    }
}

impl ChainSource for FallbackChain {
    fn open(&self, store: ChainStore) -> Result<(), String> {
        self.primary.open(store)
    }

    fn status(&self) -> ChainStatus {
        let active = self.use_fallback();
        let mut status = if active { self.fallback.status() } else { self.primary.status() };
        if active {
            // Addresses stay on the filters, so their scan is the wallet's progress.
            let primary = self.primary.status();
            status.scanned_height = primary.scanned_height;
            status.syncing = primary.syncing;
            status.error = primary.error.or(status.error);
        }
        status.fallback = Some("public-esplora");
        status.fallback_active = active || self.public_broadcast.load(Ordering::Relaxed);
        status
    }

    fn tip_height(&self) -> Result<u32, String> {
        self.pick().tip_height()
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        // Even while the fallback is answering for the tip, a public server
        // is not told this address. That query is what would link sales.
        self.primary.address_txs(address)
    }

    fn script_txs(&self, script: &bitcoin::Script, from: u32) -> Result<Vec<AddressTx>, String> {
        self.primary.script_txs(script, from)
    }

    fn fee_rate(&self) -> Result<f64, String> {
        self.pick().fee_rate()
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        if self.use_fallback() {
            return self.fallback.broadcast(tx);
        }
        match self.primary.broadcast(tx) {
            Ok(()) => Ok(()),
            // The sync gate above answers whether the tip is moving, and a client
            // with no live peer can still look healthy by that measure, which is how
            // a payment died with no peer taking the transaction while a working
            // public server was never asked. Only a transaction nobody took is
            // offered on: a peer that answered and refused it has judged it, and a
            // dead proxy would only add this client's timeouts to every retry. The
            // clearnet refusal is a configuration answer, not a delivery failure, so
            // it goes back to the caller untouched.
            Err(peer) => {
                if !crate::chain_filters::undelivered(&peer) || self.primary.proxy_is_down() {
                    return Err(peer);
                }
                eprintln!("ob-node: no filter peer took the transaction ({peer}); asking the public Esplora servers, which see a Tor exit");
                let public = self.fallback.broadcast(tx);
                if public.is_ok() {
                    self.public_broadcast.store(true, Ordering::Relaxed);
                }
                public.map_err(|public| {
                    format!("{peer}; the public Esplora servers did not take it either: {public}")
                })
            }
        }
    }

    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        self.primary.wallet_update(request)
    }

    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String> {
        self.primary.output(outpoint)
    }

    fn transaction(&self, txid: &bitcoin::Txid) -> Result<Option<(Transaction, Option<u32>)>, String> {
        self.primary.transaction(txid)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use bitcoin::{Address, Network};
    use std::str::FromStr;

    use super::*;
    use crate::chain::ChainSource;
    use crate::chain_filters::{ChainStore, FilterChain, FilterConfig};
    use crate::harness::TempDir;
    use crate::vault::Vault;

    #[test]
    fn a_quiet_filter_peer_does_not_send_sale_addresses_to_public_esplora() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 500 No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });

        let dir = TempDir::new();
        std::fs::create_dir_all(dir.0.join("filters")).unwrap();
        let primary = FilterChain::new(FilterConfig::own_node(Network::Regtest, vec![("127.0.0.1".into(), 1)])).unwrap();
        let store = ChainStore {
            dir: dir.0.join("filters"),
            vault: Vault::open(&dir.0, &crate::VaultMode::File).unwrap(),
            wallet_tip: None,
        };
        primary.open(store).unwrap();
        primary.test_peer_silent_for(Duration::from_secs(180));
        let fallback = crate::chain::EsploraChain::with_egress(
            &[format!("http://127.0.0.1:{port}")],
            Network::Regtest,
            &Egress::default(),
        )
        .unwrap();
        let chain = FallbackChain { primary, fallback, active: AtomicBool::new(false), public_broadcast: AtomicBool::new(false) };
        let address = Address::from_str("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080")
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap();

        let _ = chain.address_txs(&address);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the sale address was sent to Esplora");

        let _ = chain.tip_height();
        let started = std::time::Instant::now();
        while hits.load(Ordering::SeqCst) == 0 && started.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(hits.load(Ordering::SeqCst) >= 1, "the tip query should still use the fallback");
        let status = chain.status();
        assert!(status.fallback_active);
        assert_eq!((status.scanned_height, status.syncing), (Some(0), true), "the filter scan, not an empty height");
    }

    #[test]
    fn a_healthy_peer_that_is_quiet_between_blocks_does_not_turn_the_fallback_on() {
        use crate::chain_filters::mock_peer::{MockPeer, TestChain};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = stream.read(&mut [0u8; 1024]);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n9");
            }
        });
        let mut blocks = TestChain::new();
        blocks.mine(vec![]);
        let peer = MockPeer::start(blocks);
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.0.join("filters")).unwrap();
        let primary =
            FilterChain::new(FilterConfig::own_node(Network::Regtest, vec![(peer.addr.ip().to_string(), peer.addr.port())])).unwrap();
        primary
            .open(ChainStore { dir: dir.0.join("filters"), vault: Vault::open(&dir.0, &crate::VaultMode::File).unwrap(), wallet_tip: None })
            .unwrap();
        let fallback =
            crate::chain::EsploraChain::with_egress(&[format!("http://127.0.0.1:{port}")], Network::Regtest, &Egress::default()).unwrap();
        let chain = FallbackChain { primary, fallback, active: AtomicBool::new(false), public_broadcast: AtomicBool::new(false) };
        let started = std::time::Instant::now();
        while chain.status().syncing {
            assert!(started.elapsed() < Duration::from_secs(60), "the scan did not catch up");
            thread::sleep(Duration::from_millis(50));
        }
        let before = hits.load(Ordering::SeqCst);

        // 22 minutes without a block, asked for the tip twice a minute.
        chain.primary.test_peer_silent_for(Duration::from_secs(180));
        let tips: Vec<Result<u32, String>> = (0..44).map(|_| chain.tip_height()).collect();
        assert_eq!(hits.load(Ordering::SeqCst) - before, 0, "44 tip reads went to the public server");
        assert!(tips.iter().all(|tip| *tip == Ok(1)), "{tips:?}");
        assert!(!chain.status().fallback_active);
    }
    /// A payment that no peer takes must still reach the public server. The sync
    /// gate answers whether the tip is moving, which is not the question a
    /// broadcast asks: a client with no live peer can look healthy by that
    /// measure and still fail every payment.
    #[test]
    fn a_broadcast_the_filter_route_cannot_take_reaches_the_public_server() {
        use bitcoin::consensus::encode::deserialize_hex;
        use bitcoin::{absolute::LockTime, transaction::Version, Amount, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Witness};

        let tx = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            }],
            output: vec![TxOut { value: Amount::from_sat(1_000), script_pubkey: ScriptBuf::new() }],
        };
        let genesis = bitcoin::blockdata::constants::genesis_block(Network::Regtest).block_hash().to_string();
        let txid = tx.compute_txid().to_string();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = bodies.clone();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buf[..read]),
                    }
                }
                let head_end = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|at| at + 4)
                    .unwrap_or(request.len());
                let head = String::from_utf8_lossy(&request[..head_end]).to_lowercase();
                let path = head.lines().next().and_then(|line| line.split(' ').nth(1)).unwrap_or("").to_string();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                while request.len() < head_end + length {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buf[..read]),
                    }
                }
                let body = String::from_utf8_lossy(&request[head_end..(head_end + length).min(request.len())]).to_string();
                // The client probes GET /block-height/0 for the genesis hash, then
                // POSTs /tx with the transaction. Answer both honestly.
                let reply = if path.starts_with("/block-height/0") {
                    genesis.clone()
                } else {
                    if path.starts_with("/tx") {
                        seen.lock().unwrap().push(body);
                    }
                    txid.clone()
                };
                let _ = stream.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", reply.len(), reply).as_bytes(),
                );
            }
        });

        // The pair production actually builds: a public filter client with a
        // proxy, and a public fallback. The client is never opened here, so it
        // has no requester and fails at once; a running client with no peer fails
        // the same way after BROADCAST_TIMEOUT. The sync gate says the filters are
        // fine, which is the state that failed on a quiet chain with no peers.
        let mut config = FilterConfig::public(Network::Regtest);
        config.proxy = Some("127.0.0.1:9050".parse().unwrap());
        let primary = FilterChain::new(config).unwrap();
        let fallback = crate::chain::EsploraChain::with_egress(
            &[format!("http://127.0.0.1:{port}")],
            Network::Regtest,
            &Egress::default(),
        )
        .unwrap();
        let chain = FallbackChain { primary, fallback, active: AtomicBool::new(false), public_broadcast: AtomicBool::new(false) };
        assert!(!chain.status().fallback_active, "the sync gate is off, which is the state that failed");

        let result = chain.broadcast(&tx);
        assert!(result.is_ok(), "a payment the peers cannot take must still be relayed: {result:?}");
        let sent = bodies.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "the public server was not asked exactly once");
        let decoded: bitcoin::Transaction = deserialize_hex(sent[0].trim()).expect("the server was sent a serialized transaction");
        assert_eq!(decoded.compute_txid(), tx.compute_txid(), "a different transaction was sent than the one paid with");
    }

    /// A peer that answered and refused the transaction has judged it, and a dead
    /// proxy would only add timeouts to every retry. Neither is a reason to ask a
    /// public server, which is why the classification exists at all.
    #[test]
    fn only_an_undelivered_transaction_is_offered_to_a_public_server() {
        use crate::chain_filters::{undelivered, NOT_CONNECTED, NO_PEER_TOOK_IT};
        assert!(undelivered(NO_PEER_TOOK_IT));
        assert!(undelivered(NOT_CONNECTED));
        assert!(!undelivered("dust output: below the minimum"));
        assert!(!undelivered("a public filter peer is not sent this transaction without --proxy; it would see this machine's IP"));
    }
}
