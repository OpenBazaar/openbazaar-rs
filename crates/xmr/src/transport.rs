//! HTTP to one monerod, with a fixed list of what may be asked.
//!
//! Only the daemon routes a full wallet needs are sent. Light-wallet routes
//! (`login`, `get_address_info`, `get_address_txs`, `get_unspent_outs`,
//! `import_wallet_request`) take the private view key; they are not in the
//! list, so no code path can send a key to a server. `generateblocks` is
//! refused too.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use monero_daemon_rpc::HttpTransport;
use monero_wallet::interface::InterfaceError;
use serde_json::Value;

/// Every route the wallet may POST to.
pub const ROUTES: &[&str] = &[
    "json_rpc",
    "get_height",
    "get_transactions",
    "send_raw_transaction",
    "get_blocks.bin",
    "get_o_indexes.bin",
    "get_outs.bin",
    "get_output_distribution.bin",
];

/// Every JSON-RPC method the wallet may call through `json_rpc`.
pub const JSON_RPC_METHODS: &[&str] = &["on_get_block_hash", "get_block", "get_fee_estimate"];

/// Routes that fetch the chain every wallet downloads: they share one Tor
/// circuit per node, so a scan does not build a circuit for each request (an
/// onion service took about 9 s per block that way). Decoy and broadcast
/// routes still get a circuit of their own.
const CHAIN_ROUTES: &[&str] = &["json_rpc", "get_height", "get_transactions", "get_blocks.bin", "get_o_indexes.bin"];

fn shares_circuit(url: &reqwest::Url) -> bool {
    url.path_segments().and_then(|mut segments| segments.next_back()).is_some_and(|route| CHAIN_ROUTES.contains(&route))
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Through Tor, connecting includes the SOCKS request, and a first circuit to
/// an onion service (descriptor fetch, introduction, rendezvous) often takes
/// 40-60 s. Later requests reuse the chain circuit and are fast.
const TOR_CONNECT_TIMEOUT: Duration = Duration::from_secs(75);
/// Long enough for a first onion circuit (often 40-60 s) and four blocks.
/// A stalled daemon must not hold the scan for five minutes.
const TOR_REQUEST_TIMEOUT: Duration = Duration::from_secs(75);

/// Refuse anything outside [`ROUTES`] and [`JSON_RPC_METHODS`].
pub fn check_request(route: &str, body: &[u8]) -> Result<(), String> {
    if !ROUTES.contains(&route) {
        return Err(format!("the Monero client does not send {route}"));
    }
    if route != "json_rpc" {
        return Ok(());
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| "json_rpc body is not JSON".to_string())?;
    let calls = match &value {
        Value::Array(calls) => calls.iter().collect::<Vec<_>>(),
        call => vec![call],
    };
    for call in calls {
        let method = call.get("method").and_then(Value::as_str).unwrap_or("");
        if !JSON_RPC_METHODS.contains(&method) {
            return Err(format!("the Monero client does not call {method:?}"));
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct RpcTransport {
    url: String,
    client: reqwest::Client,
    /// Bounds the whole exchange, including a body that stalls after the headers.
    /// reqwest's own timeout ends when the headers arrive.
    request_timeout: Duration,
    /// SOCKS isolation for chain fetches. Bumped after a failure so Tor builds
    /// a new circuit instead of reusing the one that just stalled.
    circuit: Arc<AtomicU64>,
}

impl RpcTransport {
    /// `proxy` is `socks5h://HOST:PORT`. Chain fetches share one SOCKS
    /// username per transport ([`CHAIN_ROUTES`]); every other connection gets
    /// its own, so Tor puts it on its own circuit. None is kept alive.
    ///
    /// `deadline` bounds the whole exchange. A node that accepts and then
    /// stays silent has to be given up so the next daemon can be asked.
    pub fn new(url: &str, proxy: Option<&str>) -> Result<Self, String> {
        let (connect, request) =
            if proxy.is_some() { (TOR_CONNECT_TIMEOUT, TOR_REQUEST_TIMEOUT) } else { (CONNECT_TIMEOUT, REQUEST_TIMEOUT) };
        Self::bounded(url, proxy, connect, request)
    }

    pub(crate) fn bounded(url: &str, proxy: Option<&str>, connect: Duration, deadline: Duration) -> Result<Self, String> {
        Self::build(url, proxy, connect, deadline, deadline)
    }

    /// `deadline` bounds the whole exchange, including a SOCKS connect or a body
    /// that stalls. `reqwest_timeout` is the client's own timer, which does not
    /// cover every stall, so `deadline` is the one the wallet relies on.
    fn build(url: &str, proxy: Option<&str>, connect: Duration, reqwest_timeout: Duration, deadline: Duration) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let circuit = Arc::new(AtomicU64::new(NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut builder = reqwest::Client::builder()
            .connect_timeout(connect)
            .timeout(reqwest_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0);
        builder = match proxy {
            Some(proxy) => {
                let authority = proxy.trim_start_matches("socks5h://").to_string();
                let session = std::process::id();
                let circuit = Arc::clone(&circuit);
                builder.proxy(reqwest::Proxy::custom(move |url| {
                    let stream = if shares_circuit(url) {
                        chain_stream(circuit.load(Ordering::Relaxed))
                    } else {
                        NEXT.fetch_add(1, Ordering::Relaxed).to_string()
                    };
                    Some(socks_proxy(&authority, session, &stream))
                }))
            }
            None => builder.no_proxy(),
        };
        let client = builder.build().map_err(|err| err.to_string())?;
        Ok(Self { url: url.trim_end_matches('/').to_string(), client, request_timeout: deadline, circuit })
    }

    /// Drop the chain circuit. The next chain fetch uses a new SOCKS username,
    /// which makes Tor build a new circuit to this daemon.
    pub(crate) fn rotate_circuit(&self) {
        self.circuit.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn chain_generation(&self) -> u64 {
        self.circuit.load(Ordering::Relaxed)
    }
}

fn chain_stream(generation: u64) -> String {
    format!("chain{generation}")
}

fn socks_proxy(authority: &str, session: u32, stream: &str) -> String {
    format!("socks5h://obx-{session}-{stream}:x@{authority}")
}

impl HttpTransport for RpcTransport {
    fn post(
        &self,
        route: &str,
        body: Vec<u8>,
        response_size_limit: Option<usize>,
    ) -> impl Send + std::future::Future<Output = Result<Vec<u8>, InterfaceError>> {
        let checked = check_request(route, &body);
        let request = self.client.post(format!("{}/{route}", self.url)).body(body);
        let timeout = self.request_timeout;
        async move {
            checked.map_err(InterfaceError::InternalError)?;
            let fetch = async {
                let mut response = request
                    .send()
                    .await
                    .map_err(|err| InterfaceError::InterfaceError(format!("monerod: {}", err.without_url())))?;
                if !response.status().is_success() {
                    return Err(InterfaceError::InterfaceError(format!("monerod answered {}", response.status())));
                }
                let mut out = Vec::new();
                while let Some(chunk) =
                    response.chunk().await.map_err(|err| InterfaceError::InterfaceError(format!("monerod: {}", err.without_url())))?
                {
                    out.extend_from_slice(&chunk);
                    if response_size_limit.is_some_and(|limit| out.len() > limit) {
                        return Err(InterfaceError::InterfaceError("monerod sent more than was asked for".into()));
                    }
                }
                Ok(out)
            };
            match tokio::time::timeout(timeout, fetch).await {
                Ok(result) => result,
                Err(_) => Err(InterfaceError::InterfaceError("monerod: timed out".into())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_wallet_routes_and_mining_are_refused() {
        for route in ["login", "get_address_info", "get_address_txs", "get_unspent_outs", "import_wallet_request", "get_random_outs"] {
            assert!(check_request(route, b"{}").is_err(), "{route}");
        }
        let mine = br#"{"jsonrpc":"2.0","id":0,"method":"generateblocks","params":{}}"#;
        assert!(check_request("json_rpc", mine).is_err());
        let batch = br#"[{"method":"on_get_block_hash","params":[0]},{"method":"generateblocks"}]"#;
        assert!(check_request("json_rpc", batch).is_err());
        let fine = br#"[{"method":"on_get_block_hash","params":[0]},{"method":"get_block","params":{"height":1}}]"#;
        assert!(check_request("json_rpc", fine).is_ok());
        assert!(check_request("get_outs.bin", &[1, 2, 3]).is_ok());
    }

    #[test]
    fn only_chain_fetches_share_a_circuit() {
        let url = |route: &str| reqwest::Url::parse(&format!("http://abc.onion:38089/{route}")).unwrap();
        for route in CHAIN_ROUTES {
            assert!(shares_circuit(&url(route)), "{route}");
        }
        for route in ["get_outs.bin", "get_output_distribution.bin", "send_raw_transaction"] {
            assert!(ROUTES.contains(&route));
            assert!(!shares_circuit(&url(route)), "{route} reveals a spend and keeps its own circuit");
        }
    }

    #[test]
    fn a_failed_chain_circuit_gets_a_new_isolation_name() {
        let transport = RpcTransport::build(
            "http://node.onion:38089",
            Some("socks5h://127.0.0.1:9"),
            Duration::from_millis(50),
            Duration::from_millis(50),
            Duration::from_millis(50),
        )
        .unwrap();
        let first = chain_stream(transport.chain_generation());
        let before = socks_proxy("127.0.0.1:9", 7, &first);
        transport.rotate_circuit();
        let second = chain_stream(transport.chain_generation());
        let after = socks_proxy("127.0.0.1:9", 7, &second);
        assert_ne!(before, after);
        assert!(before.contains("chain") && after.contains("chain"));
    }

    #[test]
    fn a_stalled_body_does_not_hold_the_client() {
        use std::io::Read;
        use monero_daemon_rpc::HttpTransport;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut buf = [0u8; 2048];
            // Read the request, then hold the socket open and send nothing.
            // Reqwest's own timer is set long below, so only the wallet's
            // deadline can unblock this.
            let _ = stream.read(&mut buf);
            let _ = stream.read(&mut buf);
        });
        ready_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let transport = RpcTransport::build(
            &format!("http://{address}"),
            None,
            Duration::from_millis(200),
            Duration::from_secs(30),
            Duration::from_millis(400),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let started = std::time::Instant::now();
        let err = runtime
            .block_on(async { HttpTransport::post(&transport, "get_height", Vec::new(), None).await })
            .expect_err("the body never arrives");
        let elapsed = started.elapsed();
        assert!(format!("{err:?}").contains("timed out"), "{err:?} after {elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "a stalled monerod held the client for {elapsed:?}");
    }
}
