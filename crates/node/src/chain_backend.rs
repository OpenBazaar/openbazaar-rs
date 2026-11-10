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

use std::net::{SocketAddr, ToSocketAddrs};
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
                "compact block filters from P2P peers; public Esplora ({}) for the tip, fees, and broadcasts only through --proxy while no peer answers",
                fallback.join(", ")
            ),
            ChainPlan::Esplora { urls } => format!("Esplora at {}", urls.join(", ")),
        }
    }

    /// With a proxy in `egress`, filter peers, broadcasts and Esplora queries all go through it.
    pub fn build(&self, network: NetworkMode, egress: &Egress) -> Result<Box<dyn ChainSource>, String> {
        let filters = |config: &FilterConfig| {
            FilterChain::new(FilterConfig {
                proxy: egress.proxy.as_deref().map(socks_address).transpose()?,
                ..config.clone()
            })
        };
        Ok(match self {
            ChainPlan::Filters { config, fallback } if fallback.is_empty() => Box::new(filters(config)?),
            ChainPlan::Filters { config, fallback } => Box::new(FallbackChain {
                primary: filters(config)?,
                fallback: EsploraChain::with_egress(fallback, network.bitcoin(), egress)?,
                active: AtomicBool::new(false),
            }),
            ChainPlan::Esplora { urls } => Box::new(EsploraChain::with_egress(urls, network.bitcoin(), egress)?),
        })
    }
}

/// The filter client takes the proxy as a socket address; the proxy host itself is resolved here.
fn socks_address(proxy: &str) -> Result<SocketAddr, String> {
    let authority = proxy.trim_start_matches("socks5h://");
    authority
        .to_socket_addrs()
        .map_err(|err| format!("proxy {authority}: {err}"))?
        .next()
        .ok_or_else(|| format!("proxy {authority} has no address"))
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
}

impl FallbackChain {
    fn use_fallback(&self) -> bool {
        let unreachable = self.primary.unreachable_for(FALLBACK_AFTER);
        if self.active.swap(unreachable, Ordering::Relaxed) != unreachable {
            if unreachable {
                eprintln!("ob-node: WARNING: no filter peer answered or the filter scan stopped moving; public Esplora is asked for the tip, fees, and broadcasts only when --proxy is set. Addresses stay on the filters, so a payment is not seen until a peer answers");
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
        status.fallback = Some("public-esplora");
        status.fallback_active = active;
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
        self.pick().broadcast(tx)
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
        let chain = FallbackChain { primary, fallback, active: AtomicBool::new(false) };
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
    }
}
