//! HTTP to one monerod, with a fixed list of what may be asked.
//!
//! Only the daemon routes a full wallet needs are sent. Light-wallet routes
//! (`login`, `get_address_info`, `get_address_txs`, `get_unspent_outs`,
//! `import_wallet_request`) take the private view key; they are not in the
//! list, so no code path can send a key to a server. `generateblocks` is
//! refused too.

use std::sync::atomic::{AtomicU64, Ordering};
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

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

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
}

impl RpcTransport {
    /// `proxy` is `socks5h://HOST:PORT`. Each connection gets its own SOCKS
    /// username, so Tor puts it on its own circuit, and none is kept alive.
    pub fn new(url: &str, proxy: Option<&str>) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0);
        builder = match proxy {
            Some(proxy) => {
                let authority = proxy.trim_start_matches("socks5h://").to_string();
                let session = std::process::id();
                builder.proxy(reqwest::Proxy::custom(move |_| {
                    let stream = NEXT.fetch_add(1, Ordering::Relaxed);
                    Some(format!("socks5h://obx-{session}-{stream}:x@{authority}"))
                }))
            }
            None => builder.no_proxy(),
        };
        let client = builder.build().map_err(|err| err.to_string())?;
        Ok(Self { url: url.trim_end_matches('/').to_string(), client })
    }
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
        async move {
            checked.map_err(InterfaceError::InternalError)?;
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
}
