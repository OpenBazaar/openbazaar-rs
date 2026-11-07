//! A local regtest stack for tests with real LDK nodes: one `bitcoind` and
//! one LSP (an LDK node selling LSPS2 just-in-time channels).
//!
//! Needs a `bitcoind` binary: `$BITCOIND_EXE`, else `bitcoind` on `PATH`.
//! [`Bitcoind::start`] returns `None` without one, so tests can skip.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ldk_node::bitcoin::Network;
use ldk_node::config::Config;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::liquidity::LSPS2ServiceConfig;
use ldk_node::{Builder, Node};
use rand::RngCore;

use crate::config::{ChainSource, GossipSource, LnConfig, LspConfig, PeerAddress};

const USER: &str = "ob";
const PASSWORD: &str = "regtest";

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").and_then(|listener| listener.local_addr()).map(|addr| addr.port()).expect("a free port")
}

pub struct Bitcoind {
    child: Child,
    pub rpc_port: u16,
    _dir: tempfile::TempDir,
}

fn bitcoind_exe() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("BITCOIND_EXE") {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).map(|dir| dir.join("bitcoind")).find(|path| path.is_file())
    })
}

impl Bitcoind {
    /// Start a fresh regtest `bitcoind` with a funded wallet (101 blocks mined).
    pub fn start() -> Option<Self> {
        let exe = bitcoind_exe()?;
        let dir = tempfile::tempdir().ok()?;
        let rpc_port = free_port();
        let child = Command::new(exe)
            .arg("-regtest")
            .arg(format!("-datadir={}", dir.path().display()))
            .arg(format!("-rpcport={rpc_port}"))
            .arg(format!("-port={}", free_port()))
            .arg(format!("-rpcuser={USER}"))
            .arg(format!("-rpcpassword={PASSWORD}"))
            .args(["-listen=0", "-fallbackfee=0.0002", "-txindex=1", "-server=1", "-printtoconsole=0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let bitcoind = Self { child, rpc_port, _dir: dir };
        let started = Instant::now();
        while bitcoind.rpc("getblockchaininfo", "[]").is_err() {
            assert!(started.elapsed() < Duration::from_secs(30), "bitcoind did not answer");
            std::thread::sleep(Duration::from_millis(200));
        }
        bitcoind.rpc("createwallet", "[\"miner\"]").expect("a wallet");
        bitcoind.mine(101);
        Some(bitcoind)
    }

    pub fn chain(&self) -> ChainSource {
        ChainSource::BitcoindRpc { host: "127.0.0.1".into(), port: self.rpc_port, user: USER.into(), password: PASSWORD.into() }
    }

    /// One JSON-RPC call; returns `result`.
    pub fn rpc(&self, method: &str, params: &str) -> Result<serde_json::Value, String> {
        let body = format!("{{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"{method}\",\"params\":{params}}}");
        let auth = base64_basic(&format!("{USER}:{PASSWORD}"));
        let mut stream = TcpStream::connect(("127.0.0.1", self.rpc_port)).map_err(|err| err.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|err| err.to_string())?;
        let request = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Basic {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).map_err(|err| err.to_string())?;
        let mut response = String::new();
        stream.read_to_string(&mut response).map_err(|err| err.to_string())?;
        let json = response.split_once("\r\n\r\n").map(|(_, body)| body).ok_or("no body")?;
        let value: serde_json::Value = serde_json::from_str(json).map_err(|err| format!("{err}: {json}"))?;
        if !value["error"].is_null() {
            return Err(value["error"].to_string());
        }
        Ok(value["result"].clone())
    }

    pub fn mine(&self, blocks: u32) {
        let address = self.rpc("getnewaddress", "[]").expect("an address");
        self.rpc("generatetoaddress", &format!("[{blocks}, {address}]")).expect("blocks");
    }

    pub fn send(&self, address: &str, btc: f64) -> String {
        self.rpc("sendtoaddress", &format!("[\"{address}\", {btc}]")).expect("a payment").as_str().unwrap_or_default().to_string()
    }

    pub fn height(&self) -> u32 {
        self.rpc("getblockcount", "[]").ok().and_then(|value| value.as_u64()).unwrap_or(0) as u32
    }
}

impl Drop for Bitcoind {
    fn drop(&mut self) {
        let _ = self.rpc("stop", "[]");
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
    }
}

fn base64_basic(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// An LDK node selling LSPS2 just-in-time channels, funded with 1 BTC.
pub struct Lsp {
    pub node: Arc<Node>,
    pub peer: PeerAddress,
    pub fee_ppm: u32,
}

impl Lsp {
    pub fn start(bitcoind: &Bitcoind, dir: &Path) -> Self {
        let port = free_port();
        let fee_ppm = 10_000;
        let mut config = Config::default();
        config.network = Network::Regtest;
        config.storage_dir_path = dir.to_string_lossy().into_owned();
        config.listening_addresses = Some(vec![SocketAddress::TcpIpV4 { addr: [127, 0, 0, 1], port }]);
        let mut builder = Builder::from_config(config);
        let mut seed = [0u8; 64];
        rand::rng().fill_bytes(&mut seed);
        builder.set_entropy_seed_bytes(seed);
        let ChainSource::BitcoindRpc { host, port: rpc, user, password } = bitcoind.chain() else { unreachable!() };
        builder.set_chain_source_bitcoind_rpc(host, rpc, user, password);
        builder.set_liquidity_provider_lsps2(LSPS2ServiceConfig {
            require_token: None,
            advertise_service: false,
            channel_opening_fee_ppm: fee_ppm,
            // The LSP's side of a JIT channel pays its reserve, the anchors, and the
            // commitment fee; 10 % extra is too little for a 20 000-sat payment.
            channel_over_provisioning_ppm: 1_000_000,
            min_channel_opening_fee_msat: 0,
            min_channel_lifetime: 100,
            max_client_to_self_delay: 1024,
            min_payment_size_msat: 0,
            max_payment_size_msat: 1_000_000_000,
            client_trusts_lsp: true,
        });
        let node = Arc::new(builder.build().expect("the LSP node"));
        node.start().expect("the LSP starts");
        let address = node.onchain_payment().new_address().expect("an address").to_string();
        bitcoind.send(&address, 1.0);
        bitcoind.mine(6);
        let peer = PeerAddress { node_id: node.node_id(), address: format!("127.0.0.1:{port}") };
        let lsp = Self { node, peer, fee_ppm };
        wait("the LSP's funds", Duration::from_secs(60), || {
            let _ = lsp.node.sync_wallets();
            lsp.node.list_balances().spendable_onchain_balance_sats > 0
        });
        lsp.spawn_event_drain();
        lsp
    }

    pub fn config(&self) -> LspConfig {
        LspConfig { peer: self.peer.clone(), token: None }
    }

    fn spawn_event_drain(&self) {
        let node = Arc::clone(&self.node);
        std::thread::spawn(move || loop {
            match node.next_event() {
                Some(_) => {
                    if node.event_handled().is_err() {
                        return;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        });
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.node.stop();
    }
}

/// An [`LnConfig`] on regtest against `bitcoind`, with `lsp` (or none).
pub fn config(bitcoind: &Bitcoind, dir: &Path, lsp: Option<LspConfig>) -> LnConfig {
    LnConfig {
        network: Network::Regtest,
        storage_dir: dir.to_path_buf(),
        chain: bitcoind.chain(),
        gossip: GossipSource::P2p,
        lsp,
        listen: None,
        max_lsp_fee_bps: 500,
        min_lsp_fee_cap_sats: 5_000,
        proxy: None,
    }
}

/// Poll `ready` until it holds, or panic after `limit`.
pub fn wait(what: &str, limit: Duration, mut ready: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < limit, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}
