//! [`ZecChain`] over the lightwalletd gRPC protocol, which Zaino serves too.
//!
//! Four calls only: `GetLightdInfo` on every new connection, `GetLatestBlock`,
//! `GetBlock`, and `GetBlockRange` with no pool filter (Sapling, Orchard, and
//! Ironwood). No address, transaction id, or key is ever sent. A server must
//! be on this network and speak lightwallet-protocol 0.5.0 or later: an older
//! one leaves Ironwood actions out of its compact blocks, so every payment
//! would be missing without an error.
//!
//! Servers are tried in the order given; a server that fails is skipped until
//! the others fail too.
//!
//! The transport is built here, not by tonic: TCP, or SOCKS5 to `--proxy`
//! with a username of its own per connection (so Tor puts it on its own
//! circuit) and the host name resolved by the proxy; then TLS for `https`
//! (webpki roots, ring, ALPN `h2`); then HTTP/2. One connection per server is
//! reused for up to ten minutes, then a new one, on a new circuit, replaces it.

use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;
use tonic::transport::{Channel, Endpoint};
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zcash_client_backend::proto::service::{BlockId, BlockRange, ChainSpec, Empty, LightdInfo};
use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

use crate::backend::{IndexerConfig, Server, ServerKind};
use crate::proto::CompactBlock;
use crate::{BackendStatus, ZecChain, ZecNetwork};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// A connection older than this is replaced, on a new Tor circuit.
const RENEW: Duration = Duration::from_secs(10 * 60);
/// lightwallet-protocol 0.5.0 (2026-06-30) added Ironwood actions to compact blocks.
const MIN_PROTOCOL: (u64, u64, u64) = (0, 5, 0);

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Client = CompactTxStreamerClient<Channel>;

struct Slot {
    server: Server,
    /// `--proxy`, when this server is reached through it.
    proxy: Option<String>,
    channel: Mutex<Option<(Instant, Channel)>>,
}

#[derive(Default)]
struct Seen {
    tip: Option<u64>,
    active: Option<String>,
    software: Option<String>,
    error: Option<String>,
}

pub struct IndexerChain {
    config: IndexerConfig,
    runtime: tokio::runtime::Runtime,
    slots: Vec<Slot>,
    current: AtomicUsize,
    seen: Mutex<Seen>,
    tls: Arc<ClientConfig>,
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

impl IndexerChain {
    pub fn new(config: IndexerConfig) -> Result<Self, String> {
        for server in &config.servers {
            if server.kind != ServerKind::Own && config.proxy.is_none() {
                return Err(format!("{} is not contacted without --proxy; a remote Zcash indexer would see this machine's IP", server.url));
            }
        }
        Self::build(config)
    }

    /// [`IndexerChain::new`] without the Tor rule, for a test run by hand.
    fn build(config: IndexerConfig) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("ob-zec")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let slots = config
            .servers
            .iter()
            .map(|server| Slot {
                server: server.clone(),
                proxy: config.proxied(server).then(|| config.proxy.clone()).flatten(),
                channel: Mutex::new(None),
            })
            .collect();
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|err| err.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        Ok(Self { config, runtime, slots, current: AtomicUsize::new(0), seen: Mutex::new(Seen::default()), tls: Arc::new(tls) })
    }

    /// The slot's connection, made and checked if there is none or it is old.
    async fn channel(&self, slot: &Slot) -> Result<Channel, String> {
        if let Some((at, channel)) = guard(&slot.channel).clone() {
            if at.elapsed() < RENEW {
                return Ok(channel);
            }
        }
        let server = slot.server.clone();
        let proxy = slot.proxy.clone();
        let tls = Arc::clone(&self.tls);
        let connector = tower::service_fn(move |_: http::Uri| {
            let (server, proxy, tls) = (server.clone(), proxy.clone(), Arc::clone(&tls));
            async move { dial(&server, proxy.as_deref(), &tls).await.map(TokioIo::new) }
        });
        let authority = authority(&slot.server);
        let scheme = if slot.server.tls { "https" } else { "http" };
        // tonic dials `http://` through the connector, which does any TLS
        // itself; the origin carries the real scheme in each request.
        let origin: http::Uri = format!("{scheme}://{authority}").parse().map_err(|_| format!("{} is not a valid URL", slot.server.url))?;
        let endpoint = Endpoint::from_shared(format!("http://{authority}"))
            .map_err(|err| err.to_string())?
            .origin(origin)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        let channel = endpoint.connect_with_connector(connector).await.map_err(|err| failed(&slot.server, &err))?;
        let info = Client::new(channel.clone())
            .get_lightd_info(Empty {})
            .await
            .map_err(|status| format!("{}: {}", slot.server.url, status.message()))?
            .into_inner();
        check_info(&info, self.config.network, &slot.server.url)?;
        guard(&self.seen).software = Some(software(&info));
        *guard(&slot.channel) = Some((Instant::now(), channel.clone()));
        Ok(channel)
    }

    /// Run `call` against the current server, then each other one, until one answers.
    fn call<T, F, Fut>(&self, call: F) -> Result<T, String>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let count = self.slots.len();
        let start = self.current.load(Ordering::Relaxed);
        let mut last = None;
        for step in 0..count {
            let index = (start + step) % count;
            let slot = &self.slots[index];
            let result = self.runtime.block_on(async {
                let channel = self.channel(slot).await?;
                call(Client::new(channel)).await.map_err(|status| format!("{}: {}", slot.server.url, status.message()))
            });
            match result {
                Ok(value) => {
                    self.current.store(index, Ordering::Relaxed);
                    let mut seen = guard(&self.seen);
                    seen.active = Some(slot.server.url.clone());
                    seen.error = None;
                    return Ok(value);
                }
                Err(err) => {
                    *guard(&slot.channel) = None;
                    last = Some(err);
                }
            }
        }
        let err = last.unwrap_or_else(|| "no Zcash indexer is configured".to_string());
        guard(&self.seen).error = Some(err.clone());
        Err(err)
    }
}

impl ZecChain for IndexerChain {
    fn status(&self) -> BackendStatus {
        let seen = guard(&self.seen);
        BackendStatus {
            config: Some(self.config.clone()),
            network: self.config.network,
            tip: seen.tip,
            active: seen.active.clone(),
            software: seen.software.clone(),
            error: seen.error.clone(),
            mock: false,
        }
    }

    fn tip(&self) -> Result<u64, String> {
        let tip = self.call(|mut client| async move { Ok(client.get_latest_block(ChainSpec {}).await?.into_inner().height) })?;
        guard(&self.seen).tip = Some(tip);
        Ok(tip)
    }

    fn block_hash(&self, height: u64) -> Result<[u8; 32], String> {
        let hash = self.call(|mut client| async move {
            Ok(client.get_block(BlockId { height, hash: Vec::new() }).await?.into_inner().hash)
        })?;
        hash.as_slice().try_into().map_err(|_| "the Zcash indexer sent a block hash that is not 32 bytes".to_string())
    }

    fn blocks(&self, from: u64, to: u64) -> Result<Vec<CompactBlock>, String> {
        if from > to {
            return Ok(Vec::new());
        }
        let want = to - from + 1;
        self.call(|mut client| async move {
            let range = BlockRange {
                start: Some(BlockId { height: from, hash: Vec::new() }),
                end: Some(BlockId { height: to, hash: Vec::new() }),
                // Empty: the shielded pools, Ironwood included.
                pool_types: Vec::new(),
            };
            let mut stream = client.get_block_range(range).await?.into_inner();
            let mut blocks = Vec::new();
            while let Some(block) = stream.message().await? {
                blocks.push(block);
                if blocks.len() as u64 > want {
                    return Err(tonic::Status::out_of_range("the Zcash indexer sent more blocks than were asked for"));
                }
            }
            Ok(blocks)
        })
    }
}

/// The server's network, Sapling activation height, and protocol version.
fn check_info(info: &LightdInfo, network: ZecNetwork, url: &str) -> Result<(), String> {
    if info.chain_name != network.chain_name() {
        return Err(format!("{url} serves the Zcash {:?} chain, not {network}", info.chain_name));
    }
    let sapling = network.params().activation_height(NetworkUpgrade::Sapling).map(u64::from).unwrap_or(0);
    if info.sapling_activation_height != sapling {
        return Err(format!("{url} is not on Zcash {network}: its Sapling activation height is {}", info.sapling_activation_height));
    }
    if protocol_version(&info.lightwallet_protocol_version) < MIN_PROTOCOL {
        return Err(format!(
            "{url} speaks lightwallet-protocol {:?}; Ironwood payments need 0.5.0 or later, and an older server leaves them out",
            info.lightwallet_protocol_version
        ));
    }
    Ok(())
}

/// `v0.5.0`, `0.5.0`, or `0.5.0-rc.1` as numbers. Missing parts are zero.
fn protocol_version(text: &str) -> (u64, u64, u64) {
    let mut parts = text.trim().trim_start_matches('v').split(['.', '-', '+']).map(|part| part.parse::<u64>().ok());
    let mut next = || parts.next().flatten().unwrap_or(0);
    (next(), next(), next())
}

fn software(info: &LightdInfo) -> String {
    let name = if info.vendor.is_empty() { "indexer" } else { info.vendor.as_str() };
    format!("{name} {}, lightwallet-protocol {}", info.version, info.lightwallet_protocol_version)
}

fn authority(server: &Server) -> String {
    if server.host.contains(':') {
        format!("[{}]:{}", server.host, server.port)
    } else {
        format!("{}:{}", server.host, server.port)
    }
}

/// The error and its causes, each once: a cause already in the text is not repeated.
fn failed(server: &Server, err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text = format!("{text}: {cause_text}");
        }
        source = cause.source();
    }
    format!("{}: {text}", server.url)
}

/// TCP or SOCKS5, then TLS when the server is `https`.
async fn dial(server: &Server, proxy: Option<&str>, tls: &Arc<ClientConfig>) -> io::Result<Box<dyn Io>> {
    let tcp = match proxy {
        Some(proxy) => socks5(proxy, &server.host, server.port).await?,
        None => TcpStream::connect((server.host.as_str(), server.port)).await?,
    };
    tcp.set_nodelay(true)?;
    if !server.tls {
        return Ok(Box::new(tcp));
    }
    let name = ServerName::try_from(server.host.clone()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "not a TLS server name"))?;
    let stream = TlsConnector::from(Arc::clone(tls)).connect(name, tcp).await?;
    if stream.get_ref().1.alpn_protocol() != Some(b"h2") {
        return Err(io::Error::other("the indexer did not agree to HTTP/2 over TLS"));
    }
    Ok(Box::new(stream))
}

/// Connect to `host:port` through the SOCKS5 proxy at `proxy`
/// (`socks5h://HOST:PORT`). The proxy resolves the name. Each connection
/// sends a username of its own, which Tor takes as a reason for a new circuit.
async fn socks5(proxy: &str, host: &str, port: u16) -> io::Result<TcpStream> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let refused = |what: &str| io::Error::other(format!("the proxy {what}"));
    let address = proxy.trim().trim_start_matches("socks5h://").trim_start_matches("socks5://").trim_end_matches('/');
    let mut stream = TcpStream::connect(address).await?;
    // Offer username/password (for isolation), and no authentication for a proxy without it.
    stream.write_all(&[5, 2, 2, 0]).await?;
    let mut reply = [0u8; 2];
    stream.read_exact(&mut reply).await?;
    match reply {
        [5, 2] => {
            let user = format!("obz-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.extend_from_slice(&[1, b'x']);
            stream.write_all(&auth).await?;
            stream.read_exact(&mut reply).await?;
            if reply[1] != 0 {
                return Err(refused("refused the connection's username"));
            }
        }
        [5, 0] => {}
        _ => return Err(refused("does not speak SOCKS5")),
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend_from_slice(&ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let name = u8::try_from(host.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "host name too long"))?;
            request.push(3);
            request.push(name);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != 5 || head[1] != 0 {
        return Err(refused(&format!("could not reach {host}:{port} (SOCKS reply {})", head[1])));
    }
    let bound = match head[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            usize::from(len[0])
        }
        _ => return Err(refused("sent an unknown address type")),
    };
    let mut rest = vec![0u8; bound + 2];
    stream.read_exact(&mut rest).await?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(chain: &str, protocol: &str) -> LightdInfo {
        LightdInfo { chain_name: chain.into(), sapling_activation_height: 280_000, lightwallet_protocol_version: protocol.into(), ..Default::default() }
    }

    #[test]
    fn servers_must_be_on_this_network_and_carry_ironwood() {
        let url = "http://127.0.0.1:9067";
        assert!(check_info(&info("test", "v0.5.0"), ZecNetwork::Testnet, url).is_ok());
        assert!(check_info(&info("test", "0.6.0"), ZecNetwork::Testnet, url).is_ok());
        assert!(check_info(&info("main", "v0.6.0"), ZecNetwork::Testnet, url).unwrap_err().contains("\"main\" chain"));
        let regtest = LightdInfo { sapling_activation_height: 1, ..info("test", "v0.6.0") };
        assert!(check_info(&regtest, ZecNetwork::Testnet, url).unwrap_err().contains("Sapling activation height"));
        for old in ["v0.4.1", "", "0.4.0", "nonsense"] {
            assert!(check_info(&info("test", old), ZecNetwork::Testnet, url).unwrap_err().contains("Ironwood"), "{old:?}");
        }
        assert_eq!(protocol_version("v0.5.0-rc.1"), (0, 5, 0));
        assert_eq!(protocol_version("1.2"), (1, 2, 0));
    }

    /// A public testnet indexer, directly or through `OB_ZEC_LIVE_PROXY`:
    ///
    /// ```text
    /// OB_ZEC_LIVE_INDEXER=https://testnet.zec.rocks:443 cargo test -p openbazaar-zec live_testnet_indexer -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "talks to a public Zcash testnet indexer"]
    fn live_testnet_indexer() {
        let url = std::env::var("OB_ZEC_LIVE_INDEXER").unwrap_or_else(|_| "https://testnet.zec.rocks:443".into());
        let proxy = std::env::var("OB_ZEC_LIVE_PROXY").ok();
        let mut config = IndexerConfig::new(ZecNetwork::Testnet, &[url], Some("socks5h://127.0.0.1:9050".into())).unwrap();
        config.proxy = proxy;
        let chain = IndexerChain::build(config).unwrap();
        let tip = chain.tip().unwrap();
        let status = chain.status();
        eprintln!("tip {tip}, {}", status.software.as_deref().unwrap_or("?"));
        let blocks = chain.blocks(tip - 20, tip).unwrap();
        assert_eq!(blocks.len(), 21);
        assert!(blocks.windows(2).all(|pair| pair[1].height == pair[0].height + 1 && pair[1].prev_hash == pair[0].hash));
        assert_eq!(chain.block_hash(tip).unwrap().to_vec(), blocks[20].hash);
        let metadata = blocks[20].chain_metadata.expect("tree sizes");
        eprintln!("tree sizes at {tip}: orchard {} ironwood {}", metadata.orchard_commitment_tree_size, metadata.ironwood_commitment_tree_size);
        let keys = crate::ViewKey::from_seed(&[9u8; 64], ZecNetwork::Testnet).unwrap();
        let mut wallet = crate::ZecWallet::new(ZecNetwork::Testnet, keys);
        while !wallet.sync_step(&chain, 50).unwrap().caught_up() {}
        assert_eq!(wallet.scanned(), wallet.tip());
        assert!(wallet.unspent().is_empty(), "a fresh key has nothing");
    }
}
