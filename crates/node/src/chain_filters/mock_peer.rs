//! Offline stand-ins for the Bitcoin network: a regtest chain with real
//! proof of work and BIP 158 filters, a P2P peer that serves it over the v1
//! protocol, and a SOCKS5 relay that records every destination it is asked for.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::absolute::LockTime;
use bitcoin::bip158::{BlockFilter, FilterHash, FilterHeader};
use bitcoin::block::{Header, Version as BlockVersion};
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::p2p::message_blockdata::{GetHeadersMessage, Inventory};
use bitcoin::p2p::message_filter::{CFHeaders, CFilter, GetCFHeaders, GetCFilters};
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Address as P2pAddress, ServiceFlags};
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, Block, BlockHash, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};

/// A regtest chain from genesis. Blocks carry a coinbase plus whatever the test adds.
#[derive(Clone)]
pub(crate) struct TestChain {
    pub blocks: Vec<Block>,
}

impl TestChain {
    pub fn new() -> Self {
        Self { blocks: vec![genesis_block(Network::Regtest)] }
    }

    pub fn tip(&self) -> u32 {
        (self.blocks.len() - 1) as u32
    }

    /// Mine one block holding `txs` on top of the tip. Returns its height.
    pub fn mine(&mut self, txs: Vec<Transaction>) -> u32 {
        let height = self.blocks.len() as u32;
        let prev = self.blocks.last().expect("genesis").header;
        let coinbase = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes([&[0x04][..], &height.to_le_bytes(), &[0x51]].concat()),
                sequence: Sequence::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut { value: Amount::from_sat(50_000), script_pubkey: ScriptBuf::from_bytes(vec![0x51]) }],
        };
        let mut block = Block {
            header: Header {
                version: BlockVersion::from_consensus(0x2000_0000),
                prev_blockhash: prev.block_hash(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: prev.time + 600,
                bits: prev.bits,
                nonce: 0,
            },
            txdata: [vec![coinbase], txs].concat(),
        };
        block.header.merkle_root = block.compute_merkle_root().expect("transactions");
        while block.header.validate_pow(block.header.target()).is_err() {
            block.header.nonce += 1;
        }
        self.blocks.push(block);
        height
    }

    /// Drop every block above `height`.
    pub fn truncate(&mut self, height: u32) {
        self.blocks.truncate(height as usize + 1);
    }

    pub fn hash(&self, height: u32) -> BlockHash {
        self.blocks[height as usize].block_hash()
    }

    pub fn height_of(&self, hash: &BlockHash) -> Option<u32> {
        self.blocks.iter().position(|block| block.block_hash() == *hash).map(|at| at as u32)
    }

    /// The BIP 158 basic filter of the block at `height`.
    pub fn filter(&self, height: u32) -> Vec<u8> {
        let outputs: HashMap<OutPoint, ScriptBuf> = self
            .blocks
            .iter()
            .flat_map(|block| block.txdata.iter())
            .flat_map(|tx| {
                let txid = tx.compute_txid();
                tx.output
                    .iter()
                    .enumerate()
                    .map(move |(vout, out)| (OutPoint { txid, vout: vout as u32 }, out.script_pubkey.clone()))
            })
            .collect();
        let block = &self.blocks[height as usize];
        BlockFilter::new_script_filter(block, |outpoint| Ok(outputs.get(outpoint).cloned().unwrap_or_default()))
            .expect("filter")
            .content
    }

    fn filter_headers(&self) -> Vec<FilterHeader> {
        let mut previous = FilterHeader::all_zeros();
        (0..=self.tip())
            .map(|height| {
                previous = BlockFilter::new(&self.filter(height)).filter_header(&previous);
                previous
            })
            .collect()
    }
}

/// A payment of `sats` to `script` from a coin outside the chain.
pub(crate) fn pay(script: &ScriptBuf, sats: u64, tag: u8) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint { txid: Txid::from_byte_array([tag; 32]), vout: 0 },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut { value: Amount::from_sat(sats), script_pubkey: script.clone() }],
    }
}

/// Spend `outpoint` to `script`.
pub(crate) fn spend(outpoint: OutPoint, script: &ScriptBuf, sats: u64) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut { value: Amount::from_sat(sats), script_pubkey: script.clone() }],
    }
}

#[derive(Default)]
struct PeerState {
    connections: usize,
    received: Vec<Transaction>,
    blocks_served: Vec<BlockHash>,
    writers: Vec<TcpStream>,
}

/// A regtest node that serves `chain`: headers, filter headers, filters, blocks.
#[derive(Clone)]
pub(crate) struct MockPeer {
    pub addr: SocketAddr,
    chain: Arc<Mutex<TestChain>>,
    state: Arc<Mutex<PeerState>>,
}

impl MockPeer {
    pub fn start(chain: TestChain) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = Self {
            addr: listener.local_addr().unwrap(),
            chain: Arc::new(Mutex::new(chain)),
            state: Arc::default(),
        };
        let serving = peer.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let peer = serving.clone();
                thread::spawn(move || peer.serve(stream));
            }
        });
        peer
    }

    pub fn connections(&self) -> usize {
        self.state.lock().unwrap().connections
    }

    pub fn received(&self) -> Vec<Transaction> {
        self.state.lock().unwrap().received.clone()
    }

    pub fn blocks_served(&self) -> Vec<BlockHash> {
        self.state.lock().unwrap().blocks_served.clone()
    }

    /// Change the chain and announce the new tip headers above `from` to every connection.
    pub fn update(&self, from: u32, change: impl FnOnce(&mut TestChain)) {
        let headers: Vec<Header> = {
            let mut chain = self.chain.lock().unwrap();
            change(&mut chain);
            chain.blocks[from as usize + 1..].iter().map(|block| block.header).collect()
        };
        let message = frame(NetworkMessage::Headers(headers));
        for writer in &mut self.state.lock().unwrap().writers {
            let _ = writer.write_all(&message);
        }
    }

    fn serve(&self, mut stream: TcpStream) {
        let mut writer = stream.try_clone().unwrap();
        {
            let mut state = self.state.lock().unwrap();
            state.connections += 1;
            state.writers.push(stream.try_clone().unwrap());
        }
        while let Some(message) = read_message(&mut stream) {
            for reply in self.answer(message) {
                if writer.write_all(&frame(reply)).is_err() {
                    return;
                }
            }
        }
    }

    fn answer(&self, message: NetworkMessage) -> Vec<NetworkMessage> {
        let chain = self.chain.lock().unwrap().clone();
        match message {
            NetworkMessage::Version(_) => vec![NetworkMessage::Version(version(chain.tip())), NetworkMessage::Verack],
            NetworkMessage::Ping(nonce) => vec![NetworkMessage::Pong(nonce)],
            NetworkMessage::GetHeaders(GetHeadersMessage { locator_hashes, .. }) => {
                let from = locator_hashes.iter().find_map(|hash| chain.height_of(hash)).unwrap_or(0);
                let headers = chain.blocks[from as usize + 1..].iter().take(2000).map(|block| block.header).collect();
                vec![NetworkMessage::Headers(headers)]
            }
            NetworkMessage::GetCFHeaders(GetCFHeaders { start_height, stop_hash, .. }) => {
                let Some(stop) = chain.height_of(&stop_hash) else { return vec![] };
                let headers = chain.filter_headers();
                let previous = if start_height == 0 { FilterHeader::all_zeros() } else { headers[start_height as usize - 1] };
                let filter_hashes = (start_height..=stop).map(|height| FilterHash::hash(&chain.filter(height))).collect();
                vec![NetworkMessage::CFHeaders(CFHeaders {
                    filter_type: 0,
                    stop_hash,
                    previous_filter_header: previous,
                    filter_hashes,
                })]
            }
            NetworkMessage::GetCFilters(GetCFilters { start_height, stop_hash, .. }) => {
                let Some(stop) = chain.height_of(&stop_hash) else { return vec![] };
                (start_height..=stop)
                    .map(|height| {
                        NetworkMessage::CFilter(CFilter {
                            filter_type: 0,
                            block_hash: chain.hash(height),
                            filter: chain.filter(height),
                        })
                    })
                    .collect()
            }
            NetworkMessage::GetData(items) => items
                .into_iter()
                .filter_map(|item| match item {
                    Inventory::Block(hash) | Inventory::WitnessBlock(hash) => {
                        let height = chain.height_of(&hash)?;
                        self.state.lock().unwrap().blocks_served.push(hash);
                        Some(NetworkMessage::Block(chain.blocks[height as usize].clone()))
                    }
                    _ => None,
                })
                .collect(),
            NetworkMessage::Inv(items) => {
                let wanted: Vec<Inventory> = items.into_iter().filter(|item| matches!(item, Inventory::WTx(_))).collect();
                if wanted.is_empty() {
                    vec![]
                } else {
                    vec![NetworkMessage::GetData(wanted)]
                }
            }
            NetworkMessage::Tx(tx) => {
                self.state.lock().unwrap().received.push(tx);
                vec![]
            }
            _ => vec![],
        }
    }
}

fn version(height: u32) -> VersionMessage {
    let services = ServiceFlags::NETWORK | ServiceFlags::WITNESS | ServiceFlags::COMPACT_FILTERS;
    let nowhere = P2pAddress::new(&"127.0.0.1:0".parse().unwrap(), ServiceFlags::NONE);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut message = VersionMessage::new(services, now, nowhere.clone(), nowhere, 7, "/mock:0/".into(), height as i32);
    message.version = 70016;
    message
}

fn frame(message: NetworkMessage) -> Vec<u8> {
    serialize(&RawNetworkMessage::new(Network::Regtest.magic(), message))
}

fn read_message(stream: &mut TcpStream) -> Option<NetworkMessage> {
    let mut header = [0u8; 24];
    stream.read_exact(&mut header).ok()?;
    let len = u32::from_le_bytes(header[16..20].try_into().unwrap()) as usize;
    let mut bytes = header.to_vec();
    bytes.resize(24 + len, 0);
    stream.read_exact(&mut bytes[24..]).ok()?;
    let raw: RawNetworkMessage = deserialize(&bytes).ok()?;
    Some(raw.payload().clone())
}

/// A SOCKS5 server that records each CONNECT destination and relays only to `allowed`.
pub(crate) struct SocksRecorder {
    pub addr: SocketAddr,
    pub seen: Arc<Mutex<Vec<String>>>,
}

impl SocksRecorder {
    pub fn start(allowed: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                thread::spawn(move || relay(stream, allowed, &log));
            }
        });
        Self { addr, seen }
    }
}

fn relay(mut client: TcpStream, allowed: SocketAddr, seen: &Mutex<Vec<String>>) -> Option<()> {
    let mut greeting = [0u8; 2];
    client.read_exact(&mut greeting).ok()?;
    let mut methods = vec![0u8; greeting[1] as usize];
    client.read_exact(&mut methods).ok()?;
    client.write_all(&[5, 0]).ok()?;
    let mut request = [0u8; 4];
    client.read_exact(&mut request).ok()?;
    let host = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).ok()?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).ok()?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).ok()?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).ok()?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        _ => return None,
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).ok()?;
    let port = u16::from_be_bytes(port);
    seen.lock().unwrap().push(format!("{host}:{port}"));
    let target = (host.parse::<std::net::IpAddr>().ok(), port);
    if target != (Some(allowed.ip()), allowed.port()) {
        let _ = client.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]);
        return None;
    }
    let upstream = TcpStream::connect(allowed).ok()?;
    client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).ok()?;
    let (mut down_read, mut up_write) = (client.try_clone().ok()?, upstream.try_clone().ok()?);
    let (mut up_read, mut down_write) = (upstream, client);
    thread::spawn(move || std::io::copy(&mut down_read, &mut up_write));
    std::io::copy(&mut up_read, &mut down_write).ok()?;
    Some(())
}
