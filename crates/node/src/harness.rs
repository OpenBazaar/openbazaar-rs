//! Test harness: an offline chain, temporary data dirs, request helpers, and
//! several nodes in one process on their own data dirs and loopback ports.
//!
//! [`MockChain`] is shared by every node in a test, so all of them see the
//! same tip, mempool, and blocks. Nothing here touches the internet.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bdk_wallet::chain::{BlockId, ConfirmationBlockTime, TxUpdate};
use bdk_wallet::Update;
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, BlockHash, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness,
};
use serde_json::{json, Value};

use crate::chain::{AddressTx, ChainSource, WalletRequest};
use crate::http::dispatch;
use crate::network::NetworkMode;
use crate::{load_catalog, EscrowConfig, Node, NodeConfig};
use openbazaar_protocol::{fixture_catalog, fixture_keys, Catalog, Listing, Store, StoreId};

pub(crate) const SELLERS: [&str; 3] = ["Marlow Goods", "Hale Paper", "North Kitchen"];
pub(crate) const PASSPHRASE: &str = "correct horse battery";

#[derive(Default)]
pub(crate) struct Chain {
    pub tip: Mutex<u32>,
    /// How often the tip was asked for.
    pub tip_reads: std::sync::atomic::AtomicUsize,
    pub by_script: Mutex<HashMap<ScriptBuf, Vec<AddressTx>>>,
    /// Coins sent to node wallets, and whether each is confirmed.
    pub wallet_txs: Mutex<Vec<(Transaction, bool)>>,
    pub broadcasts: Mutex<Vec<Transaction>>,
    pub fee_rate: Mutex<f64>,
    pub fail_broadcast: AtomicBool,
    /// Act like compact block filters: no lookup by txid, and a script is
    /// "still scanning" the first time it is followed.
    pub filters: AtomicBool,
    /// Scripts followed through `script_txs`, with the height asked for.
    pub followed: Mutex<Vec<(ScriptBuf, u32)>>,
}

#[derive(Clone)]
pub(crate) struct MockChain(pub Arc<Chain>);

impl MockChain {
    pub fn new() -> Self {
        let chain = Chain::default();
        *chain.tip.lock().unwrap() = 100;
        *chain.fee_rate.lock().unwrap() = 2.0;
        Self(Arc::new(chain))
    }

    pub fn tip(&self) -> u32 {
        *self.0.tip.lock().unwrap()
    }

    /// Someone outside this node pays `sats` to `address`. Returns the fake txid.
    pub fn pay_external(&self, address: &str, sats: u64) -> Txid {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&n.to_le_bytes());
        bytes[31] = 0xee;
        let txid = Txid::from_byte_array(bytes);
        let script = address_any(address).script_pubkey();
        self.0
            .by_script
            .lock()
            .unwrap()
            .entry(script)
            .or_default()
            .push(AddressTx {
                txid,
                outputs: vec![(0, sats)],
                height: None,
            });
        txid
    }

    /// Mine every mempool transaction into the next block.
    pub fn mine(&self) {
        let mut tip = self.0.tip.lock().unwrap();
        *tip += 1;
        for txs in self.0.by_script.lock().unwrap().values_mut() {
            for tx in txs.iter_mut().filter(|tx| tx.height.is_none()) {
                tx.height = Some(*tip);
            }
        }
    }

    /// A one-block reorg: an empty block replaces the newest one, so its
    /// transactions are back in the mempool.
    pub fn reorg(&self) {
        let tip = self.tip();
        for txs in self.0.by_script.lock().unwrap().values_mut() {
            for tx in txs.iter_mut().filter(|tx| tx.height == Some(tip)) {
                tx.height = None;
            }
        }
    }

    /// Give a node wallet a confirmed coin it can spend.
    pub fn fund_wallet(&self, address: &Address, sats: u64) {
        self.send_to_wallet(address, sats, true);
    }

    pub fn send_to_wallet(&self, address: &Address, sats: u64, confirmed: bool) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([n as u8 ^ 0x5a; 32]),
                    vout: n as u32,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: address.script_pubkey(),
            }],
        };
        self.0.wallet_txs.lock().unwrap().push((tx, confirmed));
    }

    pub fn broadcasts(&self) -> Vec<Transaction> {
        self.0.broadcasts.lock().unwrap().clone()
    }
}

impl ChainSource for MockChain {
    fn tip_height(&self) -> Result<u32, String> {
        self.0.tip_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.tip())
    }

    fn address_txs(&self, address: &Address) -> Result<Vec<AddressTx>, String> {
        Ok(self
            .0
            .by_script
            .lock()
            .unwrap()
            .get(&address.script_pubkey())
            .cloned()
            .unwrap_or_default())
    }

    fn script_txs(&self, script: &bitcoin::Script, from: u32) -> Result<Vec<AddressTx>, String> {
        if self.0.filters.load(Ordering::Relaxed) {
            let mut followed = self.0.followed.lock().unwrap();
            if !followed.iter().any(|(seen, _)| seen.as_script() == script) {
                followed.push((script.to_owned(), from));
                return Err("compact block filters are still scanning for a newly followed address".into());
            }
        }
        Ok(self.0.by_script.lock().unwrap().get(script).cloned().unwrap_or_default())
    }

    fn fee_rate(&self) -> Result<f64, String> {
        Ok(*self.0.fee_rate.lock().unwrap())
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), String> {
        if self.0.fail_broadcast.swap(false, Ordering::Relaxed) {
            return Err("mock relay is down".to_string());
        }
        let txid = tx.compute_txid();
        let spends = |other: &Transaction| other.input.iter().any(|old| tx.input.iter().any(|new| new.previous_output == old.previous_output));
        let replaced: HashSet<Txid> = self.broadcasts().iter().filter(|other| spends(other)).map(Transaction::compute_txid).collect();
        let mut index = self.0.by_script.lock().unwrap();
        for txs in index.values_mut() {
            txs.retain(|seen| seen.height.is_some() || !replaced.contains(&seen.txid));
        }
        for (vout, out) in tx.output.iter().enumerate() {
            let entry = index.entry(out.script_pubkey.clone()).or_default();
            entry.push(AddressTx {
                txid,
                outputs: vec![(vout as u32, out.value.to_sat())],
                height: None,
            });
        }
        self.0.broadcasts.lock().unwrap().push(tx.clone());
        Ok(())
    }

    /// Confirmed wallet coins are anchored in one fixed block on top of the wallet's own tip.
    fn wallet_update(&self, request: WalletRequest) -> Result<Update, String> {
        let txs = self.0.wallet_txs.lock().unwrap().clone();
        let tip = match &request {
            WalletRequest::Full(request) => request.chain_tip(),
            WalletRequest::Sync(request) => request.chain_tip(),
        };
        let block = BlockId {
            height: 50,
            hash: BlockHash::from_byte_array([0x50; 32]),
        };
        let mut tx_update = TxUpdate::default();
        tx_update.seen_ats = txs
            .iter()
            .map(|(tx, _)| (tx.compute_txid(), 1))
            .collect::<HashSet<_>>();
        tx_update.anchors = txs
            .iter()
            .filter(|(_, confirmed)| *confirmed)
            .map(|(tx, _)| {
                (
                    ConfirmationBlockTime {
                        block_id: block,
                        confirmation_time: 1,
                    },
                    tx.compute_txid(),
                )
            })
            .collect();
        tx_update.txs = txs.into_iter().map(|(tx, _)| Arc::new(tx)).collect();
        Ok(Update {
            tx_update,
            chain: tip.map(|tip| tip.insert(block)),
            ..Default::default()
        })
    }

    /// Outputs of broadcast transactions, with the height `mine` gave them.
    fn output(&self, outpoint: &OutPoint) -> Result<Option<(TxOut, Option<u32>)>, String> {
        if self.0.filters.load(Ordering::Relaxed) {
            return Err("compact block filters find transactions by address".into());
        }
        let Some(tx) = self
            .broadcasts()
            .into_iter()
            .find(|tx| tx.compute_txid() == outpoint.txid)
        else {
            return Ok(None);
        };
        let Some(out) = tx.output.get(outpoint.vout as usize).cloned() else {
            return Ok(None);
        };
        let height = self
            .0
            .by_script
            .lock()
            .unwrap()
            .get(&out.script_pubkey)
            .and_then(|txs| txs.iter().find(|seen| seen.txid == outpoint.txid))
            .and_then(|seen| seen.height);
        Ok(Some((out, height)))
    }

    fn transaction(&self, txid: &Txid) -> Result<Option<(Transaction, Option<u32>)>, String> {
        let tx = self
            .broadcasts()
            .into_iter()
            .chain(self.0.wallet_txs.lock().unwrap().iter().map(|(tx, _)| tx.clone()))
            .find(|tx| tx.compute_txid() == *txid);
        let Some(tx) = tx else { return Ok(None) };
        let height = tx.output.iter().find_map(|out| {
            self.0.by_script.lock().unwrap().get(&out.script_pubkey).and_then(|txs| {
                txs.iter().find(|seen| seen.txid == *txid).and_then(|seen| seen.height)
            })
        });
        Ok(Some((tx, height)))
    }
}

pub(crate) struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!("ob-test-{nanos}-{}-{n}", std::process::id())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Stand-in mainnet stores for tests: the demo listings signed again by
/// keys that are not fixtures, with mainnet payment codes. A real mainnet
/// node serves only its own store.
pub(crate) fn mainnet_test_catalog() -> Catalog {
    let demo = fixture_catalog();
    let keys = fixture_keys();
    let mut catalog = Catalog { stores: Vec::new(), moderator: demo.moderator.clone(), listings: Vec::new() };
    for store in &demo.stores {
        let label = format!("openbazaar/test/mainnet-store/{}", store.name);
        let secret = bitcoin::secp256k1::SecretKey::from_slice(
            bitcoin::hashes::sha256::Hash::hash(label.as_bytes()).as_byte_array(),
        )
        .unwrap();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let code = openbazaar_pay::SilentPaymentCode::from_keys(&keys.scan, &keys.spend, NetworkMode::Mainnet.bitcoin()).unwrap();
        let pubkey = bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret);
        let pay = crate::stores::direct_pay(pubkey, code.to_string(), &store.name, NetworkMode::Mainnet).unwrap();
        let id = StoreId::from_bytes(secret.x_only_public_key(&bitcoin::secp256k1::Secp256k1::new()).0.serialize());
        let signed = Store::sign(
            store.name.clone(),
            store.region.clone(),
            pay,
            store.rating.unwrap(),
            store.details.clone().unwrap(),
            &secret,
        )
        .unwrap();
        for listing in demo.listings.iter().filter(|listing| listing.store_id == store.id) {
            catalog.listings.push(
                Listing::sign(
                    id,
                    listing.title.clone(),
                    listing.category.clone(),
                    listing.price_sats,
                    listing.ship_from.clone(),
                    listing.description.clone(),
                    listing.seller.clone(),
                    listing.stock,
                    listing.listed_on,
                    listing.moderated,
                    listing.kind,
                    listing.product.clone(),
                    listing.shipping_sats,
                    listing.free_over_sats,
                    listing.content.expect("demo listings are labelled"),
                    &secret,
                )
                .unwrap(),
            );
        }
        catalog.stores.push(signed);
    }
    catalog.verify().unwrap();
    catalog
}

/// The catalog a test node on `network` serves.
pub(crate) fn test_catalog(network: NetworkMode) -> Catalog {
    match network {
        NetworkMode::Testnet4 => load_catalog(network).unwrap(),
        NetworkMode::Mainnet => mainnet_test_catalog(),
    }
}

pub(crate) fn open_node(
    dir: &Path,
    network: NetworkMode,
    chain: &MockChain,
    confirmations: u32,
    escrow: EscrowConfig,
) -> Node {
    let node = Node::open_with(
        dir,
        test_catalog(network),
        NodeConfig {
            network,
            confirmations,
            public_server: false,
            chain: Box::new(chain.clone()),
            escrow,
        },
    )
    .unwrap();
    unlock(&node);
    node
}

/// Set the test passphrase on a new data dir, or unlock with it.
pub(crate) fn unlock(node: &Node) {
    if node.lock.state() == crate::wallet_lock::State::Setup {
        node.setup_wallet(PASSPHRASE).unwrap();
    } else {
        node.unlock_wallet(Some(PASSPHRASE)).unwrap();
    }
}

/// Quote `id` and confirm it with the passphrase. Returns the ticket, or the failed step's status and body.
pub(crate) fn ticket(node: &Node, id: &str) -> Result<String, (String, Value)> {
    let (status, quote) = request(node, "POST", &format!("/orders/{id}/pay/quote"), "");
    if !status.contains("200") {
        return Err((status, quote));
    }
    let body = json!({"quote": quote["quote"], "passphrase": PASSPHRASE}).to_string();
    let (status, ticket) = request(node, "POST", "/spend-tickets", &body);
    if !status.contains("200") {
        return Err((status, ticket));
    }
    Ok(ticket["ticket"].as_str().unwrap().to_string())
}

/// Escrow peers that refuse every connection (the discard port). Checkout
/// still records the buyer's side; the request to the vendor fails.
pub(crate) fn unreachable_escrow() -> EscrowConfig {
    EscrowConfig {
        mailbox_url: Some("http://127.0.0.1:1".into()),
        vendors: SELLERS
            .iter()
            .map(|seller| (seller.to_string(), "http://127.0.0.1:9".to_string()))
            .collect(),
        moderator_url: Some("http://127.0.0.1:9".into()),
        moderates: false,
    }
}

pub(crate) fn address_any(text: &str) -> Address {
    text.parse::<Address<_>>().unwrap().assume_checked()
}

pub(crate) fn hash_of(node: &Node, title: &str, seller: &str) -> String {
    node.catalog()
        .listings
        .iter()
        .find(|listing| listing.title == title && listing.seller == seller)
        .unwrap()
        .content_hash()
        .unwrap()
        .to_hex()
}

/// One paired loopback request through the router. Returns the status line and JSON body.
pub(crate) fn request(node: &Node, method: &str, path: &str, body: &str) -> (String, Value) {
    let raw = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4173\r\nX-Ob-Pair: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        node.pairing_token(),
        body.len(),
    );
    let response = String::from_utf8(dispatch(node, raw.as_bytes())).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.lines().next().unwrap().to_string();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

pub(crate) fn ship_to() -> Value {
    json!({"name": "Ada Brooks", "street": "14 Larch Lane", "city": "Portland", "region": "Oregon", "postalCode": "97202"})
}

/// One checkout with one package per `(title, seller, settlement)`. Returns the orders.
pub(crate) fn checkout(node: &Node, packages: &[(&str, &str, &str)]) -> Vec<Value> {
    let packages: Vec<Value> = packages
        .iter()
        .map(|(title, seller, settlement)| {
            json!({"settlement": settlement, "lines": [{"listingHash": hash_of(node, title, seller), "quantity": 1}]})
        })
        .collect();
    let body = json!({"shipTo": ship_to(), "packages": packages});
    let (status, value) = request(node, "POST", "/checkout", &body.to_string());
    assert!(status.contains("200"), "{status} {value}");
    value["orders"].as_array().unwrap().clone()
}

pub(crate) fn order(node: &Node, id: &str) -> Value {
    let (status, value) = request(node, "GET", &format!("/orders/{id}"), "");
    assert!(status.contains("200"), "{status} {value}");
    value
}

/// `POST /orders/{id}/pay`, then run the queued payment the way the pay
/// worker would. Returns the HTTP status and the order afterwards.
pub(crate) fn pay(node: &Node, id: &str, body: &str) -> (String, Value) {
    let ticket = match ticket(node, id) {
        Ok(ticket) => ticket,
        Err(failed) => return failed,
    };
    let mut body: Value = serde_json::from_str(body).unwrap();
    body["ticket"] = json!(ticket);
    let (status, value) = request(node, "POST", &format!("/orders/{id}/pay"), &body.to_string());
    if !status.contains("202") {
        return (status, value);
    }
    assert_eq!(value["paying"], true, "{value}");
    assert_eq!(node.run_pay_jobs(), 1);
    let after = order(node, id);
    assert_eq!(after["paying"], false);
    (status, after)
}

pub(crate) fn fund_node_wallet(node: &Node, chain: &MockChain, sats: u64) {
    let address = node.buyer_wallet().fresh_address().unwrap();
    chain.fund_wallet(&address, sats);
}

/// One node serving HTTP on its own loopback port and data dir. The port
/// outlives the node, so a test can restart the node behind the same URL;
/// while it is down the port answers 503 and senders keep the letter.
pub(crate) struct Peer {
    pub node: Arc<Node>,
    pub url: String,
    slot: Arc<Mutex<Option<Arc<Node>>>>,
    escrow: EscrowConfig,
    confirmations: u32,
    dir: TempDir,
}

impl Peer {
    /// Bind a free port, open the node with `escrow(own_url)`, and serve
    /// HTTP on a thread. The chain watcher, pay worker, and mail loop do not
    /// run; tests drive them so every step is deterministic.
    pub fn spawn(chain: &MockChain, escrow: impl FnOnce(String) -> EscrowConfig) -> Self {
        Self::spawn_with(chain, 1, escrow)
    }

    /// [`Self::spawn`] for a node that calls a payment final after `confirmations` blocks.
    pub fn spawn_with(chain: &MockChain, confirmations: u32, escrow: impl FnOnce(String) -> EscrowConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}");
        let dir = TempDir::new();
        let escrow = escrow(url.clone());
        let node = Arc::new(open_node(&dir.0, NetworkMode::Testnet4, chain, confirmations, escrow.clone()));
        let slot = Arc::new(Mutex::new(Some(Arc::clone(&node))));
        let serving = Arc::clone(&slot);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve_one(&serving, stream, port);
            }
        });
        Self {
            node,
            url,
            slot,
            escrow,
            confirmations,
            dir,
        }
    }

    /// Take the node down: its port answers 503 until [`Self::start`].
    pub fn stop(&self) {
        *self.slot.lock().unwrap() = None;
    }

    /// Close the node and open its data dir again, as after a crash or a
    /// restart: everything in memory is gone. `unlocked` unlocks the wallet.
    pub fn restart(&mut self, chain: &MockChain, unlocked: bool) {
        self.stop();
        let mut config = NodeConfig::offline(NetworkMode::Testnet4);
        config.chain = Box::new(chain.clone());
        config.confirmations = self.confirmations;
        config.escrow = self.escrow.clone();
        let node = Node::open_with(&self.dir.0, load_catalog(NetworkMode::Testnet4).unwrap(), config).unwrap();
        if unlocked {
            unlock(&node);
        }
        self.node = Arc::new(node);
        self.start();
    }

    pub fn start(&self) {
        *self.slot.lock().unwrap() = Some(Arc::clone(&self.node));
    }
}

fn serve_one(slot: &Mutex<Option<Arc<Node>>>, mut stream: std::net::TcpStream, port: u16) {
    use std::io::Write;
    let Ok(raw) = crate::http_parse::read(&mut stream) else {
        return;
    };
    let node = slot.lock().unwrap().clone();
    let response = match node {
        Some(node) => crate::http::respond(&node, &raw, crate::http::Client { loopback: true, port }),
        None => crate::http::http_response("503 Service Unavailable", "text/plain", b"restarting"),
    };
    let _ = stream.write_all(&response);
}

/// A buyer, a vendor, and a moderator, each on its own node, sharing one chain.
pub(crate) struct Market {
    pub chain: MockChain,
    pub buyer: Peer,
    pub vendor: Peer,
    pub moderator: Peer,
}

impl Market {
    pub fn new() -> Self {
        Self::with_confirmations(1)
    }

    /// [`Self::new`] with nodes that call a payment final after `confirmations` blocks.
    pub fn with_confirmations(confirmations: u32) -> Self {
        let chain = MockChain::new();
        let moderator = Peer::spawn_with(&chain, confirmations, |url| EscrowConfig {
            mailbox_url: Some(url),
            moderates: true,
            ..EscrowConfig::default()
        });
        let vendor = Peer::spawn_with(&chain, confirmations, |url| EscrowConfig {
            mailbox_url: Some(url),
            moderator_url: Some(moderator.url.clone()),
            ..EscrowConfig::default()
        });
        let vendors: BTreeMap<String, String> = SELLERS
            .iter()
            .map(|seller| (seller.to_string(), vendor.url.clone()))
            .collect();
        let moderator_url = moderator.url.clone();
        vendor.node.publish_store_bond("Marlow Goods").expect("the demo vendor posts a reputation bond");
        let buyer = Peer::spawn_with(&chain, confirmations, move |url| EscrowConfig {
            mailbox_url: Some(url),
            vendors,
            moderator_url: Some(moderator_url),
            moderates: false,
        });
        Self {
            chain,
            buyer,
            vendor,
            moderator,
        }
    }

    fn nodes(&self) -> [&Node; 3] {
        [&self.buyer.node, &self.vendor.node, &self.moderator.node]
    }

    /// Retry every node's outbox, then run every node's mail loop, until no
    /// node has mail left. Returns how many letters were handled.
    pub fn deliver(&self) -> usize {
        let mut total = 0;
        loop {
            for node in self.nodes() {
                node.flush_escrow_outbox(true);
            }
            let handled: usize = self
                .nodes()
                .iter()
                .map(|node| node.process_escrow_mail())
                .sum();
            if handled == 0 {
                return total;
            }
            total += handled;
        }
    }

    /// One round: retry the outboxes, and let the first node with mail
    /// handle it. Returns false when no node has mail left.
    pub fn deliver_one(&self) -> bool {
        for node in self.nodes() {
            node.flush_escrow_outbox(true);
        }
        self.nodes().iter().any(|node| node.process_escrow_mail() > 0)
    }

    /// Restart all three nodes on their data dirs, unlocked.
    pub fn restart_all(&mut self) {
        let chain = self.chain.clone();
        for peer in [&mut self.buyer, &mut self.vendor, &mut self.moderator] {
            peer.restart(&chain, true);
        }
    }

    /// Letters waiting in any node's outbox.
    pub fn queued(&self) -> usize {
        self.nodes().iter().map(|node| node.escrow.outbox.pending()).sum()
    }

    /// Mine a block and let every node's watcher see it.
    pub fn mine(&self) {
        self.chain.mine();
        for node in self.nodes() {
            node.watch_once().unwrap();
        }
    }
}
