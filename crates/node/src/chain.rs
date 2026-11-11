//! Where the node learns about the chain.
//!
//! [`ChainSource`] is the whole surface: tip height, the transactions that
//! pay one address, a fee estimate, broadcast, a BDK wallet update, and one
//! output by outpoint (for escrow funding checks).
//! [`crate::FilterChain`] (compact block filters) is the default.
//! [`EsploraChain`] talks to a list of Esplora servers. A public server is
//! asked for the tip, a fee estimate, and a broadcast. It is not asked which
//! addresses this wallet follows, or one server could link every sale.
//! A server on this machine, the LAN, or an onion is the user's own and may
//! be asked. mempool.space's own onion is still that public server: it is
//! asked for the tip, fees, and a broadcast, and not for addresses.
//! Tests use their own implementation.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use bdk_esplora::esplora_client::{self, AsyncClient};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::chain::spk_client::{FullScanRequest, SyncRequest};
use bdk_wallet::{KeychainKind, Update};
use bitcoin::constants::genesis_block;
use bitcoin::{Address, BlockHash, Network, OutPoint, Script, Transaction, TxOut, Txid};

use crate::egress::Egress;

const STOP_GAP: usize = 20;
const PARALLEL_REQUESTS: usize = 4;
const FEE_TARGET_BLOCKS: u16 = 6;
const USER_AGENT: &str = concat!("ob-node/", env!("CARGO_PKG_VERSION"));
/// Shared by every resolved address of one family, so each address gets a
/// slice of it before the next one is tried.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A new Tor circuit often needs longer than a direct TCP connect. The whole
/// request budget has to cover that circuit plus the response.
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const PROXY_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// `/block-height/0` only checks that the server is this network. It is not
/// the tip. A probe that does not answer must not use up the tip budget.
const GENESIS_PROBE: Duration = Duration::from_secs(5);
/// `network` on a server: not probed, genesis matches, wrong network, probe failed.
const NET_UNKNOWN: u8 = 0;
const NET_MATCH: u8 = 1;
const NET_REJECTED: u8 = 2;
const NET_UNPROBED: u8 = 3;
/// Returned before any request when a public server would otherwise be told an address.
const PUBLIC_ADDRESSES: &str =
    "a public Esplora server is not asked which addresses this wallet follows";
/// Returned before any request when a public server would otherwise see this machine's IP.
const PUBLIC_CLEARNET: &str =
    "a public Esplora server is not contacted without --proxy; it would see this machine's IP";

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
    /// `public`: a server someone else runs. It is not asked for addresses.
    /// Without a proxy it is not contacted. With a proxy it can see a broadcast.
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
    /// A public chain server is configured. It is not asked for addresses, and
    /// without a proxy it is not contacted.
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
    /// Transactions paying `script`. A backend that finds transactions only
    /// by the scripts it follows (compact block filters) starts following
    /// `script` from block `from`, and answers with an error until that scan
    /// is done. Escrow funding checks use this instead of [`Self::output`].
    fn script_txs(&self, script: &Script, from: u32) -> Result<Vec<AddressTx>, String>;
    /// Fee estimate in sat/vB for a few blocks out.
    fn fee_rate(&self) -> Result<f64, String>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), String>;
    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String>;
    /// The output created at `outpoint` and the block height of its
    /// transaction (`None` in the mempool), or `None` if no such output is known.
    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String>;
    /// The full transaction and its confirmation height (`None` in the mempool).
    /// `Ok(None)` means this source does not have that transaction.
    fn transaction(&self, txid: &Txid) -> Result<Option<(Transaction, Option<u32>)>, String>;
}

/// Esplora over HTTPS, trying each configured server in order.
///
/// The HTTP client tries every address a host resolves to (IPv6 and IPv4,
/// raced), so one unreachable address does not stall a query. A server whose
/// genesis block is some other network is not used. A genesis probe that
/// times out is not the tip, and it does not block `/blocks/tip/height`.
/// The last server that answered is tried first next time. With a proxy in
/// [`Egress`], every query goes through it.
pub struct EsploraChain {
    runtime: tokio::runtime::Runtime,
    servers: Vec<Server>,
    genesis: BlockHash,
    preferred: AtomicUsize,
    /// Last tip read from `/blocks/tip/height`. `GET /network` shows it.
    tip_height_seen: AtomicU32,
    tip_height_known: AtomicBool,
    /// A server someone else runs. It is not asked for addresses.
    public: bool,
    /// The HTTP client uses the configured proxy, so a public server sees an exit, not this machine.
    proxied: bool,
}

struct Server {
    url: String,
    client: AsyncClient,
    /// [`NET_UNKNOWN`], [`NET_MATCH`], [`NET_REJECTED`], or [`NET_UNPROBED`].
    network: AtomicU8,
}

/// The URL the client will dial, with the host the parser actually uses.
fn canonical_server(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url.trim())
        .map_err(|err| format!("that chain server is not a URL ({err})"))?;
    if (parsed.scheme() != "http" && parsed.scheme() != "https") || parsed.host_str().is_none() {
        return Err("a chain server must be an http or https URL with a host".into());
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

impl EsploraChain {
    pub fn new(urls: &[String], network: Network) -> Result<Self, String> {
        Self::with_egress(urls, network, &Egress::default())
    }

    pub fn with_egress(urls: &[String], network: Network, egress: &Egress) -> Result<Self, String> {
        if urls.is_empty() {
            return Err("no chain server is configured".into());
        }
        let mut http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        let (connect, request) = if egress.proxy.is_some() {
            (PROXY_CONNECT_TIMEOUT, PROXY_REQUEST_TIMEOUT)
        } else {
            (CONNECT_TIMEOUT, REQUEST_TIMEOUT)
        };
        if let Some(proxy) = egress.reqwest_proxy()? {
            http = http.proxy(proxy);
        }
        let http = http
            .user_agent(USER_AGENT)
            .connect_timeout(connect)
            .timeout(request)
            // A kept-alive connection to one of mempool.space's addresses can
            // go silent; a fresh connection races every address again.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|err| err.to_string())?;
        let servers = urls
            .iter()
            .map(|url| {
                let url = canonical_server(url)?;
                Ok(Server {
                    client: AsyncClient::from_client(url.clone(), http.clone()),
                    url,
                    network: AtomicU8::new(NET_UNKNOWN),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
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
            tip_height_seen: AtomicU32::new(0),
            tip_height_known: AtomicBool::new(false),
            public: urls.iter().any(|url| !crate::network::is_private_url(url)),
            proxied: egress.proxy.is_some(),
        })
    }

    /// A public server reached directly would see this machine's address.
    fn refuse_public_clearnet(&self) -> Result<(), String> {
        if self.public && !self.proxied {
            Err(PUBLIC_CLEARNET.into())
        } else {
            Ok(())
        }
    }

    /// Server indexes, starting with the one that last answered.
    fn order(&self) -> impl Iterator<Item = usize> + '_ {
        let start = self.preferred.load(Ordering::Relaxed);
        (0..self.servers.len()).map(move |step| (start + step) % self.servers.len())
    }

    /// Refuse a server whose chain is not this node's network.
    ///
    /// The probe is `GET /block-height/0`, the genesis hash. That is not the
    /// chain tip. One failed probe (mempool.space often times out here over
    /// Tor) must not be asked again, and it must not replace `/blocks/tip/height`.
    fn check(&self, server: &Server) -> Result<(), String> {
        match server.network.load(Ordering::Relaxed) {
            NET_MATCH | NET_UNPROBED => return Ok(()),
            NET_REJECTED => return Err("it serves a different bitcoin network".into()),
            _ => {}
        }
        let probed = self.runtime.block_on(async {
            tokio::time::timeout(GENESIS_PROBE, server.client.get_block_hash(0)).await
        });
        match probed {
            Ok(Ok(hash)) if hash == self.genesis => {
                server.network.store(NET_MATCH, Ordering::Relaxed);
                Ok(())
            }
            Ok(Ok(_)) => {
                server.network.store(NET_REJECTED, Ordering::Relaxed);
                Err("it serves a different bitcoin network".into())
            }
            Ok(Err(_)) | Err(_) => {
                server.network.store(NET_UNPROBED, Ordering::Relaxed);
                Ok(())
            }
        }
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
            self.preferred
                .store((index + 1) % self.servers.len(), Ordering::Relaxed);
        }
        result.map_err(|err| format!("{}: {err}", server.url))
    }
}

impl ChainSource for EsploraChain {
    fn status(&self) -> ChainStatus {
        let mut status = if self.public {
            ChainStatus::simple(
                "esplora",
                "public",
                "A public Esplora server is not asked which addresses this wallet follows. Without --proxy it is not contacted at all, so it does not see this machine's IP. With --proxy it sees the tip, fee estimates, and any transaction broadcast to it.",
            )
        } else {
            ChainStatus::simple(
                "esplora",
                "own-server",
                "Only your own Esplora server sees the addresses this node follows.",
            )
        };
        if self.tip_height_known.load(Ordering::Relaxed) {
            status.scanned_height = Some(self.tip_height_seen.load(Ordering::Relaxed));
        }
        status
    }

    fn tip_height(&self) -> Result<u32, String> {
        self.refuse_public_clearnet()?;
        let height = match self.read_tip() {
            Ok(height) => height,
            // A fresh Tor circuit often fails once. One more pass uses another circuit.
            Err(err) if self.proxied => {
                self.read_tip().map_err(|again| format!("{err}; {again}"))?
            }
            Err(err) => return Err(err),
        };
        self.tip_height_seen.store(height, Ordering::Relaxed);
        if !self.tip_height_known.swap(true, Ordering::Relaxed) {
            let index = self.preferred.load(Ordering::Relaxed);
            eprintln!("ob-node: chain tip {height} from {}", self.servers[index].url);
        }
        Ok(height)
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        self.script_txs(&address.script_pubkey(), 0)
    }

    /// Esplora indexes every script, so `from` is not needed. A public server
    /// is not asked: the script would tell it an address this wallet follows.
    fn script_txs(&self, script: &Script, _from: u32) -> Result<Vec<AddressTx>, String> {
        if self.public {
            return Err(PUBLIC_ADDRESSES.into());
        }
        let txs = self.each(|client| async move {
            client
                .scripthash_txs(script, None)
                .await
                .map_err(|err| err.to_string())
        })?;
        Ok(txs
            .into_iter()
            .map(|tx| AddressTx {
                txid: tx.txid,
                outputs: tx
                    .vout
                    .iter()
                    .enumerate()
                    .filter(|(_, out)| out.scriptpubkey.as_script() == script)
                    .map(|(vout, out)| (vout as u32, out.value))
                    .collect(),
                height: if tx.status.confirmed {
                    tx.status.block_height
                } else {
                    None
                },
            })
            .filter(|tx| !tx.outputs.is_empty())
            .collect())
    }

    fn fee_rate(&self) -> Result<f64, String> {
        self.refuse_public_clearnet()?;
        self.each(|client| async move {
            let estimates = client
                .get_fee_estimates()
                .await
                .map_err(|err| err.to_string())?;
            esplora_client::convert_fee_rate(FEE_TARGET_BLOCKS as usize, estimates)
                .map(f64::from)
                .ok_or_else(|| "the server returned no fee estimate".to_string())
        })
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        self.refuse_public_clearnet()?;
        self.each(|client| async move { client.broadcast(tx).await.map_err(|err| err.to_string()) })
    }

    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        if self.public {
            return Err(PUBLIC_ADDRESSES.into());
        }
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
        if self.public {
            return Err(PUBLIC_ADDRESSES.into());
        }
        let txid = outpoint.txid;
        self.each(|client| async move {
            let Some(tx) = client.get_tx(&txid).await.map_err(|err| err.to_string())? else {
                return Ok(None);
            };
            let Some(out) = tx.output.get(outpoint.vout as usize).cloned() else {
                return Ok(None);
            };
            let status = client
                .get_tx_status(&txid)
                .await
                .map_err(|err| err.to_string())?;
            Ok(Some((
                out,
                if status.confirmed {
                    status.block_height
                } else {
                    None
                },
            )))
        })
    }

    fn transaction(&self, txid: &Txid) -> Result<Option<(Transaction, Option<u32>)>, String> {
        if self.public {
            return Err(PUBLIC_ADDRESSES.into());
        }
        let txid = *txid;
        self.each(|client| async move {
            let Some(tx) = client.get_tx(&txid).await.map_err(|err| err.to_string())? else {
                return Ok(None);
            };
            let status = client
                .get_tx_status(&txid)
                .await
                .map_err(|err| err.to_string())?;
            Ok(Some((
                tx,
                if status.confirmed {
                    status.block_height
                } else {
                    None
                },
            )))
        })
    }
}

/// No chain at all. Every query fails, so status stays where it is.
pub struct OfflineChain;

impl ChainSource for OfflineChain {
    fn status(&self) -> ChainStatus {
        ChainStatus::simple(
            "offline",
            "private",
            "This node asks no one about the chain.",
        )
    }

    fn tip_height(&self) -> Result<u32, String> {
        Err(offline())
    }
    fn address_txs(&self, _: &Address) -> Result<Vec<AddressTx>, String> {
        Err(offline())
    }
    fn script_txs(&self, _: &Script, _: u32) -> Result<Vec<AddressTx>, String> {
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
    fn transaction(&self, _: &Txid) -> Result<Option<(Transaction, Option<u32>)>, String> {
        Err(offline())
    }
}

fn offline() -> String {
    "this node has no chain source".to_string()
}

impl EsploraChain {
    fn read_tip(&self) -> Result<u32, String> {
        self.each(|client| async move { fetch_tip_height(&client).await })
    }
}

/// `GET {server}/blocks/tip/height`. The body is the height in decimal.
async fn fetch_tip_height(client: &AsyncClient) -> Result<u32, String> {
    let url = format!("{}/blocks/tip/height", client.url().trim_end_matches('/'));
    let response = client
        .client()
        .get(&url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("tip height HTTP {}", response.status()));
    }
    let text = response.text().await.map_err(|err| err.to_string())?;
    parse_tip_height(&text)
}

/// Esplora returns the tip as decimal text. Surrounding whitespace is not part of the number.
fn parse_tip_height(body: &str) -> Result<u32, String> {
    let text = body.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("the tip height was not a number ({body:?})"));
    }
    text.parse::<u32>().map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tip_height_ignores_surrounding_whitespace() {
        assert_eq!(parse_tip_height("160218").unwrap(), 160218);
        assert_eq!(parse_tip_height("160218\n").unwrap(), 160218);
        assert_eq!(parse_tip_height(" 160219 ").unwrap(), 160219);
        assert!(parse_tip_height("").is_err());
        assert!(parse_tip_height("160218\n0").is_err());
        assert!(parse_tip_height("nope").is_err());
    }

    /// Records each request path and answers from `respond(path) -> (status, body)`.
    fn local_esplora(
        respond: impl Fn(&str) -> (u16, &'static str) + Send + Sync + 'static,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        let paths = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&paths);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                recorded.lock().unwrap().push(path.clone());
                let (status, body) = respond(&path);
                let reason = if status == 200 { "OK" } else { "No" };
                let message = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(message.as_bytes());
            }
        });
        (url, paths)
    }

    #[test]
    fn tip_height_is_read_from_blocks_tip_height_when_the_genesis_probe_fails() {
        let (url, paths) = local_esplora(|path| match path {
            "/blocks/tip/height" => (200, "160218\n"),
            _ => (404, "no genesis"),
        });
        let chain = EsploraChain::new(&[url], Network::Testnet4).unwrap();
        assert_eq!(chain.tip_height().unwrap(), 160218);
        assert_eq!(chain.status().scanned_height, Some(160218));
        let seen = paths.lock().unwrap().clone();
        assert!(
            seen.iter().any(|path| path == "/blocks/tip/height"),
            "the tip was not read after the genesis probe failed: {seen:?}"
        );
        assert!(
            seen.iter()
                .filter(|path| path.as_str() == "/block-height/0")
                .count()
                <= 1,
            "a failed genesis probe was repeated instead of reading the tip: {seen:?}"
        );
    }

    #[test]
    fn a_server_on_another_network_is_not_used_for_the_tip() {
        let (url, paths) = local_esplora(|path| match path {
            "/block-height/0" => (
                200,
                "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
            ),
            "/blocks/tip/height" => (200, "99"),
            _ => (404, ""),
        });
        let chain = EsploraChain::new(&[url], Network::Testnet4).unwrap();
        let err = chain.tip_height().unwrap_err();
        assert!(err.contains("different bitcoin network"), "{err}");
        assert_eq!(chain.status().scanned_height, None);
        let seen = paths.lock().unwrap().clone();
        assert!(
            seen.iter().all(|path| path == "/block-height/0"),
            "{seen:?}"
        );
    }

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
        let (kind, host) = seen
            .recv_timeout(Duration::from_secs(5))
            .expect("no query reached the proxy");
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
                if stream.read_exact(&mut greeting).is_err()
                    || stream
                        .read_exact(&mut methods[..greeting[1] as usize])
                        .is_err()
                {
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
                let Some(user) = field(&mut stream) else {
                    continue;
                };
                let _ = field(&mut stream);
                let _ = tx.send(user);
                let _ = stream.write_all(&[1, 0]);
                // The request follows; refuse it so the client gives up.
                let _ = stream.write_all(&[5, 4, 0, 1, 0, 0, 0, 0, 0, 0]);
            }
        });
        let egress = Egress {
            proxy: Some(proxy),
            live_rates: false,
        };
        let urls = ["https://mempool.space/testnet4/api".to_string()];
        let chain = EsploraChain::with_egress(&urls, Network::Testnet4, &egress).unwrap();
        assert!(chain.tip_height().is_err());
        assert!(chain.tip_height().is_err());
        let first = users.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = users.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(
            first, second,
            "a shared SOCKS username would put both queries on one circuit"
        );
    }

    #[test]
    fn a_public_esplora_server_is_not_asked_which_addresses_are_followed() {
        use bdk_wallet::Wallet;
        use bitcoin::{Address, OutPoint};
        use std::str::FromStr;

        let (proxy, seen) = crate::egress::fake_socks::start();
        let egress = Egress {
            proxy: Some(proxy),
            live_rates: false,
        };
        let urls = ["https://mempool.space/testnet4/api".to_string()];
        let chain = EsploraChain::with_egress(&urls, Network::Testnet4, &egress).unwrap();
        assert!(chain.status().reveals.contains("not asked which addresses"));
        assert!(!chain.status().reveals.contains("sees every address"));

        let address = Address::from_str("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
            .unwrap()
            .require_network(Network::Testnet4)
            .unwrap();
        let script_err = chain.script_txs(&address.script_pubkey(), 0).unwrap_err();
        let address_err = chain.address_txs(&address).unwrap_err();
        let output_err = chain.output(&OutPoint::null()).unwrap_err();
        for err in [&script_err, &address_err, &output_err] {
            assert!(err.contains("not asked which addresses"), "{err}");
        }

        let wallet = Wallet::create(
            "tr([7d94197e/86'/1'/0']tpubDCyQVJj8KzjiQsFjmb3KwECVXPvMwvAxxZGCP9XmWSopmjW3bCV3wD7TgxrUhiGSueDS1MU5X1Vb1YjYcp8jitXc5fXfdC1z68hDDEyKRNr/0/*)",
            "tr([7d94197e/86'/1'/0']tpubDCyQVJj8KzjiQsFjmb3KwECVXPvMwvAxxZGCP9XmWSopmjW3bCV3wD7TgxrUhiGSueDS1MU5X1Vb1YjYcp8jitXc5fXfdC1z68hDDEyKRNr/1/*)",
        )
        .network(Network::Testnet4)
        .create_wallet_no_persist()
        .unwrap();
        let scan_err = chain
            .wallet_update(WalletRequest::Full(wallet.start_full_scan().build()))
            .unwrap_err();
        assert!(scan_err.contains("not asked which addresses"), "{scan_err}");
        assert!(
            seen.try_recv().is_err(),
            "an address query reached the public server"
        );

        assert!(chain.tip_height().is_err());
        let (kind, host) = seen
            .recv_timeout(Duration::from_secs(5))
            .expect("the tip query did not reach the proxy");
        assert_eq!((kind, host.as_str()), (3, "mempool.space"));
    }

    fn sample_tx() -> Transaction {
        use bitcoin::{
            absolute::LockTime, transaction::Version, Amount, OutPoint, ScriptBuf, Sequence, TxIn,
            TxOut, Witness,
        };
        Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[test]
    fn a_public_esplora_broadcast_is_not_sent_in_the_clear() {
        let urls = ["http://203.0.113.9:8332".to_string()];
        let chain =
            EsploraChain::with_egress(&urls, Network::Testnet4, &Egress::default()).unwrap();
        let started = std::time::Instant::now();
        let broadcast_err = chain.broadcast(&sample_tx()).unwrap_err();
        let tip_err = chain.tip_height().unwrap_err();
        let fee_err = chain.fee_rate().unwrap_err();
        for err in [broadcast_err, tip_err, fee_err] {
            assert!(err.contains("not contacted without --proxy"), "{err}");
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "a clearnet dial was attempted"
        );

        let (proxy, seen) = crate::egress::fake_socks::start();
        let proxied = EsploraChain::with_egress(
            &["https://mempool.space/testnet4/api".to_string()],
            Network::Testnet4,
            &Egress {
                proxy: Some(proxy),
                live_rates: false,
            },
        )
        .unwrap();
        assert!(proxied.broadcast(&sample_tx()).is_err());
        let (kind, host) = seen
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the broadcast did not reach the proxy");
        assert_eq!((kind, host.as_str()), (3, "mempool.space"));
    }

    #[test]
    fn an_esplora_redirect_is_not_followed() {
        use std::io::{Read, Write};
        use std::str::FromStr;
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;

        let leak = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let leak_url = format!("http://{}", leak.local_addr().unwrap());
        let leaked = Arc::new(AtomicUsize::new(0));
        let leaked_thread = Arc::clone(&leaked);
        std::thread::spawn(move || {
            for mut stream in leak.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                leaked_thread.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nleaked",
                );
            }
        });
        let private = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let private_url = format!("http://{}", private.local_addr().unwrap());
        let asked = Arc::new(AtomicUsize::new(0));
        let asked_thread = Arc::clone(&asked);
        std::thread::spawn(move || {
            for mut stream in private.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                asked_thread.fetch_add(1, Ordering::SeqCst);
                let body = format!("see {leak_url}");
                let head = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {leak_url}/scripthash/x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        let chain =
            EsploraChain::with_egress(&[private_url], Network::Testnet4, &Egress::default())
                .unwrap();
        let address = Address::from_str("tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx")
            .unwrap()
            .require_network(Network::Testnet4)
            .unwrap();
        let err = chain.script_txs(&address.script_pubkey(), 0).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            asked.load(Ordering::SeqCst) >= 1,
            "the private server was not asked"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            leaked.load(Ordering::SeqCst),
            0,
            "a redirect left the configured server"
        );
    }
}
