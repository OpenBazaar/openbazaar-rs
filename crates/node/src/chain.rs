//! Where the node learns about the chain.
//!
//! [`ChainSource`] is the whole surface: tip height, the transactions that
//! pay one address, a fee estimate, broadcast, a BDK wallet update, and one
//! output by outpoint (for escrow funding checks).
//! [`crate::FilterChain`] (compact block filters) is the default.
//! [`EsploraChain`] talks to a list of Esplora servers. Every query names
//! addresses to that server, so it is used only when the user names a
//! server or opts in to the public ones. Tests use their own implementation.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use bdk_esplora::esplora_client::{self, AsyncClient};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::chain::spk_client::{FullScanRequest, SyncRequest};
use bdk_wallet::{KeychainKind, Update};
use bitcoin::constants::genesis_block;
use bitcoin::{Address, BlockHash, Network, OutPoint, Transaction, TxOut, Txid};

use crate::egress::Egress;

const STOP_GAP: usize = 20;
const PARALLEL_REQUESTS: usize = 4;
const FEE_TARGET_BLOCKS: u16 = 6;
const USER_AGENT: &str = concat!("ob-node/", env!("CARGO_PKG_VERSION"));
/// Shared by every resolved address of one family, so each address gets a
/// slice of it before the next one is tried.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

/// What a chain backend is and who learns the user's addresses from it. `GET /network` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainStatus {
    /// `compact-filters`, `own-node`, `esplora`, `offline`, or `test`.
    pub backend: &'static str,
    /// `private`: no one learns which addresses are the user's.
    /// `own-node`, `own-server`: only a node or server the user runs does.
    /// `public`: a server someone else runs sees every address the node follows.
    pub privacy: &'static str,
    pub reveals: String,
    pub peers: Option<usize>,
    /// Highest block whose data the backend has checked.
    pub scanned_height: Option<u32>,
    pub syncing: bool,
    pub error: Option<String>,
    /// The backend used when this one cannot reach the network.
    pub fallback: Option<&'static str>,
    pub fallback_active: bool,
}

impl ChainStatus {
    /// Addresses currently go to a server someone else runs.
    pub fn public_server(&self) -> bool {
        self.privacy == "public"
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "backend": self.backend,
            "privacy": self.privacy,
            "reveals": self.reveals,
            "peers": self.peers,
            "scannedHeight": self.scanned_height,
            "syncing": self.syncing,
            "error": self.error,
            "fallback": self.fallback,
            "fallbackActive": self.fallback_active,
        })
    }

    fn simple(backend: &'static str, privacy: &'static str, reveals: &str) -> Self {
        Self {
            backend,
            privacy,
            reveals: reveals.to_string(),
            peers: None,
            scanned_height: None,
            syncing: false,
            error: None,
            fallback: None,
            fallback_active: false,
        }
    }
}

pub trait ChainSource: Send + Sync {
    /// Called once when the node opens its data dir, before any query. A
    /// backend that keeps state of its own seals it in `store`.
    fn open(&self, _store: crate::ChainStore) -> Result<(), String> {
        Ok(())
    }
    fn status(&self) -> ChainStatus {
        ChainStatus::simple("test", "private", "Nothing leaves this process.")
    }
    fn tip_height(&self) -> Result<u32, String>;
    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String>;
    /// Fee estimate in sat/vB for a few blocks out.
    fn fee_rate(&self) -> Result<f64, String>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), String>;
    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String>;
    /// The output created at `outpoint` and the block height of its
    /// transaction (`None` in the mempool), or `None` if no such output is known.
    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String>;
}

/// Esplora over HTTPS, trying each configured server in order.
///
/// The HTTP client tries every address a host resolves to (IPv6 and IPv4,
/// raced), so one unreachable address does not stall a query. A server is
/// used only after its genesis block matches the node's network. The last
/// server that answered is tried first next time. With a proxy in
/// [`Egress`], every query goes through it.
pub struct EsploraChain {
    runtime: tokio::runtime::Runtime,
    servers: Vec<Server>,
    genesis: BlockHash,
    preferred: AtomicUsize,
    public: bool,
}

struct Server {
    url: String,
    client: AsyncClient,
    checked: AtomicBool,
}

impl EsploraChain {
    pub fn new(urls: &[String], network: Network) -> Result<Self, String> {
        Self::with_egress(urls, network, &Egress::default())
    }

    pub fn with_egress(urls: &[String], network: Network, egress: &Egress) -> Result<Self, String> {
        if urls.is_empty() {
            return Err("no chain server is configured".into());
        }
        let mut http = reqwest::Client::builder();
        if let Some(proxy) = egress.reqwest_proxy()? {
            http = http.proxy(proxy);
        }
        let http = http
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // A kept-alive connection to one of mempool.space's addresses can
            // go silent; a fresh connection races every address again.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|err| err.to_string())?;
        let servers = urls
            .iter()
            .map(|url| {
                let url = url.trim_end_matches('/').to_string();
                Server {
                    client: AsyncClient::from_client(url.clone(), http.clone()),
                    url,
                    checked: AtomicBool::new(false),
                }
            })
            .collect();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ob-chain")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self {
            runtime,
            servers,
            genesis: genesis_block(network).block_hash(),
            preferred: AtomicUsize::new(0),
            public: urls.iter().any(|url| !crate::network::is_private_url(url)),
        })
    }

    /// Server indexes, starting with the one that last answered.
    fn order(&self) -> impl Iterator<Item = usize> + '_ {
        let start = self.preferred.load(Ordering::Relaxed);
        (0..self.servers.len()).map(move |step| (start + step) % self.servers.len())
    }

    /// Refuse a server whose chain is not this node's network.
    fn check(&self, server: &Server) -> Result<(), String> {
        if server.checked.load(Ordering::Relaxed) {
            return Ok(());
        }
        let genesis = self
            .runtime
            .block_on(server.client.get_block_hash(0))
            .map_err(|err| err.to_string())?;
        if genesis != self.genesis {
            return Err("it serves a different bitcoin network".into());
        }
        server.checked.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Run one query against each server in turn until one answers.
    fn each<T, F, Fut>(&self, query: F) -> Result<T, String>
    where
        F: Fn(AsyncClient) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let mut failures = Vec::new();
        for index in self.order() {
            let server = &self.servers[index];
            let result = self
                .check(server)
                .and_then(|()| self.runtime.block_on(query(server.client.clone())));
            match result {
                Ok(value) => {
                    self.preferred.store(index, Ordering::Relaxed);
                    return Ok(value);
                }
                Err(err) => failures.push(format!("{}: {err}", server.url)),
            }
        }
        Err(failures.join("; "))
    }

    /// Wallet scans consume their request, so they use one server per call
    /// and move to the next server after a failure.
    fn scan<T>(&self, run: impl FnOnce(&AsyncClient) -> Result<T, String>) -> Result<T, String> {
        let index = self.preferred.load(Ordering::Relaxed);
        let server = &self.servers[index];
        let result = self.check(server).and_then(|()| run(&server.client));
        if result.is_err() {
            self.preferred.store((index + 1) % self.servers.len(), Ordering::Relaxed);
        }
        result.map_err(|err| format!("{}: {err}", server.url))
    }
}

impl ChainSource for EsploraChain {
    fn status(&self) -> ChainStatus {
        if self.public {
            ChainStatus::simple(
                "esplora",
                "public",
                "A public Esplora server sees every address this node follows, when it asks, and can link them into one wallet.",
            )
        } else {
            ChainStatus::simple("esplora", "own-server", "Only your own Esplora server sees the addresses this node follows.")
        }
    }

    fn tip_height(&self) -> Result<u32, String> {
        self.each(|client| async move { client.get_height().await.map_err(|err| err.to_string()) })
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        let script = address.script_pubkey();
        let txs = self.each(|client| {
            let address = address.clone();
            async move { client.get_address_txs(&address, None).await.map_err(|err| err.to_string()) }
        })?;
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
        self.each(|client| async move {
            let estimates = client.get_fee_estimates().await.map_err(|err| err.to_string())?;
            esplora_client::convert_fee_rate(FEE_TARGET_BLOCKS as usize, estimates)
                .map(f64::from)
                .ok_or_else(|| "the server returned no fee estimate".to_string())
        })
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        self.each(|client| async move { client.broadcast(tx).await.map_err(|err| err.to_string()) })
    }

    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        self.scan(|client| {
            let result = match request {
                WalletRequest::Full(request) => self
                    .runtime
                    .block_on(client.full_scan(request, STOP_GAP, PARALLEL_REQUESTS))
                    .map(Update::from),
                WalletRequest::Sync(request) => self
                    .runtime
                    .block_on(client.sync(request, PARALLEL_REQUESTS))
                    .map(Update::from),
            };
            result.map_err(|err| err.to_string())
        })
    }

    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String> {
        let txid = outpoint.txid;
        self.each(|client| async move {
            let Some(tx) = client.get_tx(&txid).await.map_err(|err| err.to_string())? else {
                return Ok(None);
            };
            let Some(out) = tx.output.get(outpoint.vout as usize).cloned() else {
                return Ok(None);
            };
            let status = client.get_tx_status(&txid).await.map_err(|err| err.to_string())?;
            Ok(Some((out, if status.confirmed { status.block_height } else { None })))
        })
    }
}

/// No chain at all. Every query fails, so status stays where it is.
pub struct OfflineChain;

impl ChainSource for OfflineChain {
    fn status(&self) -> ChainStatus {
        ChainStatus::simple("offline", "private", "This node asks no one about the chain.")
    }

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
    fn output(&self, _: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String> {
        Err(offline())
    }
}

fn offline() -> String {
    "this node has no chain source".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_queries_go_through_the_proxy_with_the_name_unresolved() {
        let (proxy, seen) = crate::egress::fake_socks::start();
        let egress = Egress {
            proxy: Some(proxy),
            live_rates: false,
        };
        let urls = ["https://mempool.space/testnet4/api".to_string()];
        let chain = EsploraChain::with_egress(&urls, Network::Testnet4, &egress).unwrap();
        assert!(chain.tip_height().is_err());
        let (kind, host) = seen.recv_timeout(Duration::from_secs(5)).expect("no query reached the proxy");
        assert_eq!((kind, host.as_str()), (3, "mempool.space"));
    }

    #[test]
    fn every_chain_query_gets_its_own_tor_circuit() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
        let (tx, users) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut greeting = [0u8; 2];
                let mut methods = [0u8; 255];
                if stream.read_exact(&mut greeting).is_err() || stream.read_exact(&mut methods[..greeting[1] as usize]).is_err() {
                    continue;
                }
                let _ = stream.write_all(&[5, 2]);
                let field = |stream: &mut std::net::TcpStream| {
                    let mut len = [0u8; 1];
                    stream.read_exact(&mut len).ok()?;
                    let mut text = vec![0u8; len[0] as usize];
                    stream.read_exact(&mut text).ok()?;
                    Some(String::from_utf8_lossy(&text).into_owned())
                };
                let mut version = [0u8; 1];
                let _ = stream.read_exact(&mut version);
                let Some(user) = field(&mut stream) else { continue };
                let _ = field(&mut stream);
                let _ = tx.send(user);
                let _ = stream.write_all(&[1, 0]);
                // The request follows; refuse it so the client gives up.
                let _ = stream.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]);
            }
        });
        let egress = Egress { proxy: Some(proxy), live_rates: false };
        let urls = ["https://mempool.space/testnet4/api".to_string()];
        let chain = EsploraChain::with_egress(&urls, Network::Testnet4, &egress).unwrap();
        assert!(chain.tip_height().is_err());
        assert!(chain.tip_height().is_err());
        let first = users.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = users.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(first, second, "a shared SOCKS username would put both queries on one circuit");
    }
}
