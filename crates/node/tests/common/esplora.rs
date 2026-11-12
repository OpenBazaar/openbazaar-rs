//! A stand-in Esplora server on loopback for real-node tests.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use bitcoin::hashes::{sha256, Hash};
use bitcoin::{Address, ScriptBuf};
use serde_json::{json, Value};

/// One payment the stand-in chain knows: who it pays, how much, and the block it is in.
struct Payment {
    txid: String,
    script: ScriptBuf,
    sats: u64,
    height: Option<u32>,
}

#[derive(Default)]
struct Chain {
    tip: u32,
    payments: Vec<Payment>,
}

/// A stand-in Esplora server with only what the node's watcher asks for:
/// the tip height and the transactions that pay a script. Every other
/// request is answered 404, so a wallet scan or a broadcast fails at once.
pub struct Esplora {
    pub url: String,
    chain: Arc<Mutex<Chain>>,
}

impl Esplora {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let chain = Arc::new(Mutex::new(Chain { tip: 100, payments: Vec::new() }));
        let served = Arc::clone(&chain);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = match answer(&served.lock().unwrap(), path) {
                    Some(body) => ("200 OK", body),
                    None => ("404 Not Found", String::new()),
                };
                let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        Self { url, chain }
    }

    /// Someone outside the node pays `sats` to `address`, unconfirmed.
    pub fn pay(&self, address: &str, sats: u64) {
        let script = address.parse::<Address<_>>().unwrap().assume_checked().script_pubkey();
        let mut chain = self.chain.lock().unwrap();
        let txid = format!("{:064x}", chain.payments.len() + 1);
        chain.payments.push(Payment { txid, script, sats, height: None });
    }

    /// Mine every unconfirmed payment into the next block.
    pub fn mine(&self) {
        let mut chain = self.chain.lock().unwrap();
        chain.tip += 1;
        let tip = chain.tip;
        for payment in chain.payments.iter_mut().filter(|payment| payment.height.is_none()) {
            payment.height = Some(tip);
        }
    }
}

/// The body for `GET path`, or `None` for a path this server does not have.
fn answer(chain: &Chain, path: &str) -> Option<String> {
    if path == "/blocks/tip/height" {
        return Some(chain.tip.to_string());
    }
    let wanted = path.strip_prefix("/scripthash/")?.strip_suffix("/txs")?;
    let paying = chain.payments.iter().filter(|payment| format!("{:x}", sha256::Hash::hash(payment.script.as_bytes())) == wanted);
    let txs: Vec<Value> = paying
        .map(|payment| {
            json!({
                "txid": payment.txid,
                "version": 2,
                "locktime": 0,
                "vin": [],
                "vout": [{ "scriptpubkey": payment.script.to_hex_string(), "value": payment.sats }],
                "size": 0,
                "weight": 0,
                "fee": 0,
                "status": { "confirmed": payment.height.is_some(), "block_height": payment.height },
            })
        })
        .collect();
    Some(Value::Array(txs).to_string())
}
