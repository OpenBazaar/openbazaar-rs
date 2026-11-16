//! `--tor`: this node's own Arti client, offered to its HTTP, filter-peer,
//! Monero, Zcash, and Lightning clients as a SOCKS5 proxy on a free loopback
//! port, so the node needs no Tor daemon on the machine.
//!
//! Only CONNECT and Tor's RESOLVE extension are served, and only on
//! 127.0.0.1. The filter-peer client looks up its seeds with RESOLVE, as
//! Tor's own SocksPort allows. A SOCKS username and
//! password become an Arti isolation key, so streams with different
//! credentials get different circuits, as Tor's SocksPort does. State and
//! cache live in their own directories, apart from the onion services' client.

use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use arti_client::config::{BoolOrAuto, TorClientConfigBuilder};
use arti_client::isolation::IsolationHelper;
use arti_client::{BootstrapBehavior, StreamPrefs, TorClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tor_rtcompat::PreferredRuntime;

/// How long a local client has to finish the SOCKS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one CONNECT may wait for bootstrapping and retries in all
/// (Tor's own SocksTimeout is two minutes).
const CONNECT_PATIENCE: Duration = Duration::from_secs(120);
const CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_RETRY_WAIT: Duration = Duration::from_secs(3);

/// What `/network` reports while this process runs built-in Tor.
static STATUS: OnceLock<Status> = OnceLock::new();

struct Status {
    client: Arc<TorClient<PreferredRuntime>>,
    failure: Arc<Mutex<Option<String>>>,
}

pub struct TorSocks {
    url: String,
    _runtime: tokio::runtime::Runtime,
}

impl TorSocks {
    /// Start the client under `dir` and listen. Bootstrapping goes on in the
    /// background; a connection made before it finishes waits for it.
    pub fn start(dir: &Path) -> Result<Self, String> {
        let (state, cache) = tormail::private_dirs(dir)
            .map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ob-tor-socks")
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        let mut builder = TorClientConfigBuilder::from_directories(state, cache);
        if std::env::var_os("OB_TOR_TRUST_DATA_DIR").is_some() {
            builder.storage().permissions().dangerously_trust_everyone();
        }
        let config = builder.build().map_err(|err| err.to_string())?;
        let (client, listener) = runtime.block_on(async {
            let client = TorClient::builder()
                .config(config)
                .bootstrap_behavior(BootstrapBehavior::OnDemand)
                .create_unbootstrapped()
                .map_err(|err| err.to_string())?;
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .map_err(|err| err.to_string())?;
            Ok::<_, String>((client, listener))
        })?;
        let url = format!(
            "socks5h://{}",
            listener.local_addr().map_err(|err| err.to_string())?
        );
        crate::remote::tor_log::install();
        let failure = Arc::new(Mutex::new(None));
        let _ = STATUS.set(Status {
            client: client.clone(),
            failure: failure.clone(),
        });
        let background = client.clone();
        runtime.spawn(async move {
            let started = Instant::now();
            match background.bootstrap().await {
                Ok(()) => eprintln!(
                    "ob-node: tor: built-in Tor connected after {}s",
                    started.elapsed().as_secs()
                ),
                Err(err) => {
                    eprintln!("ob-node: tor: built-in Tor could not connect: {err}");
                    *failure.lock().unwrap_or_else(|err| err.into_inner()) = Some(err.to_string());
                }
            }
        });
        runtime.spawn(accept(listener, client));
        Ok(Self {
            url,
            _runtime: runtime,
        })
    }

    /// `socks5h://127.0.0.1:PORT`, for `--proxy`.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Where built-in Tor's bootstrap stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bootstrap {
    /// Bootstrapped and able to build circuits, not merely listening.
    pub ready: bool,
    pub progress: u8,
    /// Why it is not ready, if Arti knows.
    pub problem: Option<String>,
    /// Bootstrapping gave up; it will not become ready on its own.
    pub failed: bool,
}

/// `None` without `--tor`.
pub(crate) fn status() -> Option<Bootstrap> {
    let status = STATUS.get()?;
    let now = status.client.bootstrap_status();
    let ready = now.ready_for_traffic();
    let failure = status
        .failure
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone();
    let problem = if ready {
        None
    } else {
        now.blocked()
            .map(|blockage| blockage.to_string())
            .or(failure.clone())
    };
    Some(Bootstrap {
        ready,
        progress: (now.as_frac() * 100.0).round() as u8,
        problem,
        failed: !ready && failure.is_some(),
    })
}

async fn accept(listener: TcpListener, client: Arc<TorClient<PreferredRuntime>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let client = client.clone();
        tokio::spawn(async move {
            let _ = relay(&client, stream).await;
        });
    }
}

/// SOCKS username and password, as an isolation key.
#[derive(Clone, PartialEq, Eq)]
struct Credentials(Vec<u8>);

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials(..)")
    }
}

impl IsolationHelper for Credentials {
    fn compatible_same_type(&self, other: &Self) -> bool {
        self == other
    }

    fn join_same_type(&self, other: &Self) -> Option<Self> {
        (self == other).then(|| self.clone())
    }
}

/// A CONNECT or RESOLVE request: where to, and the credentials it came with.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    host: String,
    port: u16,
    credentials: Option<Credentials>,
    resolve: bool,
}

const CONNECT: u8 = 1;
/// Tor's SOCKS extension: look a name up through the network, reply with the address.
const RESOLVE: u8 = 0xf0;
const SUCCEEDED: u8 = 0;
const GENERAL_FAILURE: u8 = 1;
const COMMAND_NOT_SUPPORTED: u8 = 7;
const ADDRESS_NOT_SUPPORTED: u8 = 8;
/// Not a reply code: the client does not speak SOCKS5, so hang up.
const HANG_UP: u8 = 0xff;

async fn relay(client: &TorClient<PreferredRuntime>, mut local: TcpStream) -> std::io::Result<()> {
    let request = match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut local)).await {
        Ok(Ok(request)) => request,
        Ok(Err(HANG_UP)) | Err(_) => return Ok(()),
        Ok(Err(code)) => return reply(&mut local, code).await,
    };
    let mut prefs = StreamPrefs::new();
    prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));
    if let Some(credentials) = request.credentials {
        prefs.set_isolation(credentials);
    }
    if request.resolve {
        let address = match resolve(client, &request.host, &prefs).await {
            Some(std::net::IpAddr::V4(ip)) => {
                [&[5, SUCCEEDED, 0, 1][..], &ip.octets(), &[0, 0]].concat()
            }
            Some(std::net::IpAddr::V6(ip)) => {
                [&[5, SUCCEEDED, 0, 4][..], &ip.octets(), &[0, 0]].concat()
            }
            None => return reply(&mut local, GENERAL_FAILURE).await,
        };
        return local.write_all(&address).await;
    }
    match connect(client, &request.host, request.port, &prefs).await {
        Some(mut tor) => {
            reply(&mut local, SUCCEEDED).await?;
            tokio::io::copy_bidirectional(&mut local, &mut tor)
                .await
                .map(|_| ())
        }
        None => reply(&mut local, GENERAL_FAILURE).await,
    }
}

/// Like Tor's SocksPort, a request waits for the client to finish
/// bootstrapping instead of failing while it cannot build circuits yet. On a
/// fresh install that takes up to a minute or two, and a wallet or Lightning
/// client that got an instant failure would back off or give up. A failed
/// stream is tried again a few times, all within [`CONNECT_PATIENCE`].
async fn connect(
    client: &TorClient<PreferredRuntime>,
    host: &str,
    port: u16,
    prefs: &StreamPrefs,
) -> Option<arti_client::DataStream> {
    let deadline = tokio::time::Instant::now() + CONNECT_PATIENCE;
    for attempt in 0..CONNECT_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(CONNECT_RETRY_WAIT).await;
        }
        if !matches!(
            tokio::time::timeout_at(deadline, client.bootstrap()).await,
            Ok(Ok(()))
        ) {
            return None;
        }
        match tokio::time::timeout_at(deadline, client.connect_with_prefs((host, port), prefs))
            .await
        {
            Ok(Ok(stream)) => return Some(stream),
            Ok(Err(_)) => continue,
            Err(_) => return None,
        }
    }
    None
}

/// The first address the exit finds for `host`, once bootstrapped.
async fn resolve(
    client: &TorClient<PreferredRuntime>,
    host: &str,
    prefs: &StreamPrefs,
) -> Option<std::net::IpAddr> {
    let deadline = tokio::time::Instant::now() + CONNECT_PATIENCE;
    if !matches!(
        tokio::time::timeout_at(deadline, client.bootstrap()).await,
        Ok(Ok(()))
    ) {
        return None;
    }
    match tokio::time::timeout_at(deadline, client.resolve_with_prefs(host, prefs)).await {
        Ok(Ok(addresses)) => addresses.into_iter().next(),
        _ => None,
    }
}

async fn reply(local: &mut TcpStream, code: u8) -> std::io::Result<()> {
    local.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

/// RFC 1928 greeting, RFC 1929 username/password if offered, then the
/// request. `Err` carries the reply code to send, or [`HANG_UP`].
async fn handshake<S: AsyncReadExt + AsyncWriteExt + Unpin>(local: &mut S) -> Result<Request, u8> {
    let io = |_| GENERAL_FAILURE;
    let mut head = [0u8; 2];
    local.read_exact(&mut head).await.map_err(io)?;
    if head[0] != 5 {
        return Err(HANG_UP);
    }
    let mut methods = vec![0u8; head[1] as usize];
    local.read_exact(&mut methods).await.map_err(io)?;
    let method = if methods.contains(&2) {
        2
    } else if methods.contains(&0) {
        0
    } else {
        let _ = local.write_all(&[5, 0xff]).await;
        return Err(HANG_UP);
    };
    local.write_all(&[5, method]).await.map_err(io)?;
    let credentials = if method == 2 {
        let version = local.read_u8().await.map_err(io)?;
        let user = read_short(local).await?;
        let password = read_short(local).await?;
        if version != 1 {
            return Err(HANG_UP);
        }
        local.write_all(&[1, 0]).await.map_err(io)?;
        let mut key = user;
        key.push(0);
        key.extend(password);
        Some(Credentials(key))
    } else {
        None
    };
    let mut request = [0u8; 4];
    local.read_exact(&mut request).await.map_err(io)?;
    if request[0] != 5 {
        return Err(GENERAL_FAILURE);
    }
    let host = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            local.read_exact(&mut ip).await.map_err(io)?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        3 => String::from_utf8(read_short(local).await?).map_err(|_| ADDRESS_NOT_SUPPORTED)?,
        4 => {
            let mut ip = [0u8; 16];
            local.read_exact(&mut ip).await.map_err(io)?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        _ => return Err(ADDRESS_NOT_SUPPORTED),
    };
    let port = local.read_u16().await.map_err(io)?;
    if request[1] != CONNECT && request[1] != RESOLVE {
        return Err(COMMAND_NOT_SUPPORTED);
    }
    Ok(Request {
        host,
        port,
        credentials,
        resolve: request[1] == RESOLVE,
    })
}

async fn read_short<S: AsyncReadExt + Unpin>(local: &mut S) -> Result<Vec<u8>, u8> {
    let len = local.read_u8().await.map_err(|_| GENERAL_FAILURE)?;
    let mut bytes = vec![0u8; len as usize];
    local
        .read_exact(&mut bytes)
        .await
        .map_err(|_| GENERAL_FAILURE)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(client_bytes: &[u8]) -> (Result<Request, u8>, Vec<u8>) {
        let (mut near, mut far) = tokio::io::duplex(1024);
        near.write_all(client_bytes).await.unwrap();
        let result = handshake(&mut far).await;
        drop(far);
        let mut sent = Vec::new();
        near.read_to_end(&mut sent).await.unwrap();
        (result, sent)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_connect_by_name_with_credentials_is_isolated_by_them() {
        let mut bytes = vec![5, 2, 0, 2, 1, 4];
        bytes.extend(b"ob-1");
        bytes.push(1);
        bytes.extend(b"x");
        bytes.extend([5, 1, 0, 3, 11]);
        bytes.extend(b"example.org");
        bytes.extend(443u16.to_be_bytes());
        let (request, sent) = block_on(run(&bytes));
        let request = request.unwrap();
        assert_eq!((request.host.as_str(), request.port), ("example.org", 443));
        assert_eq!(request.credentials, Some(Credentials(b"ob-1\0x".to_vec())));
        assert_eq!(
            sent,
            [5, 2, 1, 0],
            "username/password chosen, then accepted"
        );
        let other = Credentials(b"ob-2\0x".to_vec());
        assert!(!request
            .credentials
            .as_ref()
            .unwrap()
            .compatible_same_type(&other));
    }

    #[test]
    fn ip_destinations_and_no_auth_are_accepted() {
        let mut bytes = vec![5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7];
        bytes.extend(8333u16.to_be_bytes());
        let (request, sent) = block_on(run(&bytes));
        assert_eq!(
            request.unwrap(),
            Request {
                host: "10.0.0.7".into(),
                port: 8333,
                credentials: None,
                resolve: false
            }
        );
        assert_eq!(sent, [5, 0]);
        let mut bytes = vec![5, 1, 0, 5, 1, 0, 4];
        bytes.extend(std::net::Ipv6Addr::LOCALHOST.octets());
        bytes.extend(80u16.to_be_bytes());
        assert_eq!(block_on(run(&bytes)).0.unwrap().host, "::1");
    }

    /// bip157 looks its seeds up this way; refusing it left wallet sync with no peers.
    #[test]
    fn a_tor_resolve_request_is_served() {
        let mut bytes = vec![5, 1, 0, 5, 0xf0, 0, 3, 21];
        bytes.extend(b"seed.testnet4.wiz.biz");
        bytes.extend([0, 0]);
        let request = block_on(run(&bytes)).0.unwrap();
        assert!(request.resolve);
        assert_eq!(request.host, "seed.testnet4.wiz.biz");
    }

    #[test]
    fn only_socks5_connect_and_resolve_are_served() {
        assert_eq!(block_on(run(&[4, 1, 0, 80])).0, Err(HANG_UP), "SOCKS4");
        let (result, sent) = block_on(run(&[5, 1, 9]));
        assert_eq!(result, Err(HANG_UP));
        assert_eq!(sent, [5, 0xff], "no acceptable method");
        let mut bind = vec![5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4];
        bind.extend(80u16.to_be_bytes());
        assert_eq!(block_on(run(&bind)).0, Err(COMMAND_NOT_SUPPORTED));
        let mut udp = vec![5, 1, 0, 5, 3, 0, 1, 1, 2, 3, 4];
        udp.extend(80u16.to_be_bytes());
        assert_eq!(block_on(run(&udp)).0, Err(COMMAND_NOT_SUPPORTED));
        assert_eq!(
            block_on(run(&[5, 1, 0, 5, 1, 0, 9])).0,
            Err(ADDRESS_NOT_SUPPORTED)
        );
    }
}
