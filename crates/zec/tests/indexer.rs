//! The gRPC client against a small lightwalletd stand-in serving the mock
//! chain over HTTP/2: a wallet syncs and finds an Ironwood payment, a server
//! on another network or without Ironwood is refused, a dead server is
//! skipped, and a remote server is reached only through SOCKS, with a
//! username per connection and the host name left to the proxy.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use openbazaar_zec::mock::indexer::{indexer, serve, testnet_info, Indexer};
use openbazaar_zec::mock::MockChain;
use openbazaar_zec::{IndexerChain, IndexerConfig, ViewKey, ZecChain, ZecNetwork, ZecWallet, CONFIRMATIONS};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zcash_client_backend::proto::service::LightdInfo;

const NET: ZecNetwork = ZecNetwork::Testnet;
const METHODS: [&str; 4] = ["GetLightdInfo", "GetLatestBlock", "GetBlock", "GetBlockRange"];

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

fn config(urls: &[String], proxy: Option<String>) -> IndexerConfig {
    IndexerConfig::new(NET, urls, proxy).unwrap()
}

#[test]
fn a_wallet_syncs_over_grpc_and_finds_an_ironwood_payment() {
    let server = runtime();
    let chain = Arc::new(MockChain::new(NET));
    let stand_in = indexer(&chain, "v0.6.0");
    let address = serve(&server, Arc::clone(&stand_in));
    let client = IndexerChain::new(config(&[format!("http://{address}")], None)).unwrap();

    let mut wallet = ZecWallet::new(NET, ViewKey::from_seed(&[3u8; 64], NET).unwrap());
    while !wallet.sync_step(&client, 40).unwrap().caught_up() {}
    let (index, order) = wallet.issue_address();
    chain.pay(&order, 250_000);
    chain.mine(CONFIRMATIONS);
    while !wallet.sync_step(&client, 40).unwrap().caught_up() {}
    let received = wallet.received(index);
    assert_eq!((received.total, received.confirmed), (250_000, 250_000));
    assert_eq!(wallet.scanned(), Some(chain.height()));

    let status = client.status();
    assert_eq!(status.active.as_deref(), Some(format!("http://{address}").as_str()));
    assert_eq!(status.tip, Some(chain.height()));
    assert_eq!(status.software.as_deref(), Some("Stand-in 0.0.1, lightwallet-protocol v0.6.0"));
    let value = status.value();
    assert_eq!((&value["backend"], &value["privacy"]), (&serde_json::json!("own-node"), &serde_json::json!("own-node")));
    let calls = stand_in.calls.lock().unwrap();
    assert!(calls.iter().all(|call| METHODS.contains(&call.as_str())), "only block calls: {calls:?}");
    assert_eq!(calls.iter().filter(|call| *call == "GetLightdInfo").count(), 1, "one connection, checked once");
}

#[test]
fn a_server_on_another_network_or_without_ironwood_is_refused() {
    let server = runtime();
    let chain = Arc::new(MockChain::new(NET));
    let old = serve(&server, indexer(&chain, "v0.4.1"));
    let err = IndexerChain::new(config(&[format!("http://{old}")], None)).unwrap().tip().unwrap_err();
    assert!(err.contains("Ironwood payments need 0.5.0"), "{err}");

    let mainnet = Arc::new(Indexer { chain: Arc::clone(&chain), info: LightdInfo { chain_name: "main".into(), ..testnet_info("v0.6.0") }, calls: Mutex::new(Vec::new()) });
    let main = serve(&server, Arc::clone(&mainnet));
    let err = IndexerChain::new(config(&[format!("http://{main}")], None)).unwrap().tip().unwrap_err();
    assert!(err.contains("\"main\" chain"), "{err}");
    assert_eq!(*mainnet.calls.lock().unwrap(), ["GetLightdInfo"], "nothing is asked of a server on another network");
}

#[test]
fn a_dead_server_is_skipped_for_the_next() {
    let server = runtime();
    let chain = Arc::new(MockChain::new(NET));
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let live = serve(&server, indexer(&chain, "0.5.0"));
    let client = IndexerChain::new(config(&[format!("http://{dead}"), format!("http://{live}")], None)).unwrap();
    assert_eq!(client.tip().unwrap(), chain.height());
    assert_eq!(client.status().active.as_deref(), Some(format!("http://{live}").as_str()));
    assert_eq!(client.blocks(chain.height() - 4, chain.height()).unwrap().len(), 5);
    assert!(client.blocks(chain.height() + 1, chain.height() + 2).is_err(), "blocks past the tip are an error");
    assert!(client.status().error.is_some());
}

/// One SOCKS5 connection the stand-in proxy saw: the username and where it was asked to go.
type Seen = Arc<Mutex<Vec<(String, String, u16)>>>;

/// A SOCKS5 proxy that records each request and connects every one to `target`.
fn socks_proxy(runtime: &tokio::runtime::Runtime, target: SocketAddr) -> (SocketAddr, Seen) {
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let address = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    let record = Arc::clone(&seen);
    runtime.spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { return };
            let record = Arc::clone(&record);
            tokio::spawn(async move {
                let mut head = [0u8; 2];
                client.read_exact(&mut head).await.unwrap();
                let mut methods = vec![0u8; usize::from(head[1])];
                client.read_exact(&mut methods).await.unwrap();
                assert!(methods.contains(&2), "username authentication is offered");
                client.write_all(&[5, 2]).await.unwrap();
                let mut auth = [0u8; 2];
                client.read_exact(&mut auth).await.unwrap();
                let mut user = vec![0u8; usize::from(auth[1])];
                client.read_exact(&mut user).await.unwrap();
                let mut pass_len = [0u8; 1];
                client.read_exact(&mut pass_len).await.unwrap();
                let mut pass = vec![0u8; usize::from(pass_len[0])];
                client.read_exact(&mut pass).await.unwrap();
                client.write_all(&[1, 0]).await.unwrap();
                let mut request = [0u8; 4];
                client.read_exact(&mut request).await.unwrap();
                assert_eq!(request[3], 3, "the host name goes to the proxy unresolved");
                let mut len = [0u8; 1];
                client.read_exact(&mut len).await.unwrap();
                let mut host = vec![0u8; usize::from(len[0])];
                client.read_exact(&mut host).await.unwrap();
                let mut port = [0u8; 2];
                client.read_exact(&mut port).await.unwrap();
                record.lock().unwrap().push((String::from_utf8(user).unwrap(), String::from_utf8(host).unwrap(), u16::from_be_bytes(port)));
                client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                let mut upstream = TcpStream::connect(target).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    (address, seen)
}

#[test]
fn a_remote_server_is_reached_only_through_socks_with_a_username_per_connection() {
    let server = runtime();
    let chain = Arc::new(MockChain::new(NET));
    let live = serve(&server, indexer(&chain, "v0.6.0"));
    let (proxy, seen) = socks_proxy(&server, live);
    let urls = ["http://indexer.example.org:9067".to_string()];
    assert!(IndexerConfig::new(NET, &urls, None).is_err(), "a remote indexer needs --proxy");
    let proxy = Some(format!("socks5h://{proxy}"));
    let first = IndexerChain::new(config(&urls, proxy.clone())).unwrap();
    assert_eq!(first.tip().unwrap(), chain.height());
    assert_eq!(first.blocks(chain.height() - 2, chain.height()).unwrap().len(), 3);
    let second = IndexerChain::new(config(&urls, proxy)).unwrap();
    assert_eq!(second.tip().unwrap(), chain.height());

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one connection per client, reused: {seen:?}");
    assert!(seen.iter().all(|(_, host, port)| host == "indexer.example.org" && *port == 9067));
    assert!(seen.iter().all(|(user, _, _)| user.starts_with("obz-")));
    assert_ne!(seen[0].0, seen[1].0, "each connection has its own username, so Tor gives it its own circuit");
    let value = first.status().value();
    assert_eq!((&value["backend"], &value["privacy"], &value["proxied"]), (&serde_json::json!("remote-indexer"), &serde_json::json!("remote-tor"), &serde_json::json!(true)));
}
