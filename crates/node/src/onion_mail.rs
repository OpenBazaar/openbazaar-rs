//! One sealed letter between two nodes, and the public shop on that onion.
//!
//! The clearnet listener stays on loopback. The same v3 onion answers
//! `GET /public/store` and the other public routes the sale gate allows.
//! Wallet, escrow spend, and mail secrets are not among them. A peer that
//! is a v3 onion is dialed with [`tormail::ArtiOrderTransport`] and the letter
//! is stored in the vendor's `mailbox/` by [`tormail::RunningMailbox`]. A
//! peer that is `http://` on this machine is still posted to `/escrow/mail`.
//! Anything else is refused before a socket opens. Checkout still writes the
//! buyer's own copy; this module is the path that actually leaves the process.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use tormail::{ArtiOrderTransport, OnionAddress, OrderTransport, RunningMailbox, SealedLetter};

use crate::escrow_mail::check_loopback;
use crate::escrow_outbox::{HttpTransport, Refusal, Transport};
use crate::network::NetworkMode;
use crate::Node;

const READ_LIMIT: u64 = 64 * 1024;

/// Where one sealed letter is sent.
pub(crate) enum LetterRoute {
    LoopbackHttp(String),
    Onion(OnionAddress),
}

/// Classify `peer`. An onion is never passed to [`check_loopback`].
pub(crate) fn letter_route(peer: &str) -> Result<LetterRoute, String> {
    let peer = peer.trim();
    if peer.is_empty() {
        return Err("a peer address is required".into());
    }
    if !peer.contains("://") {
        let onion = OnionAddress::parse(peer).map_err(|_| format!("{peer} is not a v3 onion address"))?;
        return Ok(LetterRoute::Onion(onion));
    }
    check_loopback(peer)?;
    Ok(LetterRoute::LoopbackHttp(peer.trim_end_matches('/').to_string()))
}

trait LetterSend {
    fn loopback(&self, url: &str, letter: &SealedLetter) -> Result<(), String>;
    fn onion(&self, onion: &OnionAddress, letter: &SealedLetter) -> Result<(), String>;
}

fn send_routed(peer: &str, letter: &SealedLetter, send: &dyn LetterSend) -> Result<(), String> {
    match letter_route(peer)? {
        LetterRoute::LoopbackHttp(url) => send.loopback(&url, letter),
        LetterRoute::Onion(onion) => send.onion(&onion, letter),
    }
}

struct LiveSend<'a> {
    state_dir: &'a Path,
    timeout: Duration,
}

impl LetterSend for LiveSend<'_> {
    fn loopback(&self, url: &str, letter: &SealedLetter) -> Result<(), String> {
        HttpTransport::loopback().deliver(url, &letter.to_bytes()).map_err(|err| match err {
            Refusal::Retry(message) | Refusal::Refused(message) => message,
        })
    }

    fn onion(&self, onion: &OnionAddress, letter: &SealedLetter) -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        runtime.block_on(async {
            let transport = ArtiOrderTransport::from_state_dir(self.state_dir, onion.as_str())
                .map_err(|err| err.to_string())?;
            match tokio::time::timeout(self.timeout, transport.deliver(letter)).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(format!("dialing {onion} failed: {err}")),
                Err(_) => Err(format!("dialing {onion} timed out after {}s", self.timeout.as_secs())),
            }
        })
    }
}

/// Dial `peer` and write one sealed letter. Fails on a bad address, a refused
/// dial, or a timeout. A network failure is not success.
pub fn deliver_letter(state_dir: &Path, peer: &str, bytes: &[u8], timeout: Duration) -> Result<(), String> {
    let letter = SealedLetter::from_bytes(bytes).map_err(|err| err.to_string())?;
    send_routed(peer, &letter, &LiveSend { state_dir, timeout })
}

/// Lower-hex SHA-256 of a sealed letter's bytes, the same digest `poll-mail` prints.
pub fn letter_digest(bytes: &[u8]) -> String {
    sha256::Hash::hash(bytes).to_string()
}

/// One letter sitting in a node's sealed mailbox.
pub struct StoredLetter {
    pub id: String,
    pub sha256: String,
}

/// Letters in `dir` (`{data}/{network}/mailbox`), oldest id first.
///
/// Does not create the directory. A missing mailbox is an error, so a wrong
/// path is not reported as an empty inbox.
pub fn poll_mailbox(dir: &Path) -> Result<Vec<StoredLetter>, String> {
    if !dir.is_dir() {
        return Err(format!("no mailbox at {}", dir.display()));
    }
    let mailbox = tormail::Mailbox::open(dir).map_err(|err| err.to_string())?;
    let mut found = Vec::new();
    for id in mailbox.list().map_err(|err| err.to_string())? {
        let letter = mailbox.read(&id).map_err(|err| err.to_string())?;
        found.push(StoredLetter {
            id: id.as_str(),
            sha256: letter_digest(&letter.to_bytes()),
        });
    }
    Ok(found)
}

impl Node {
    /// Remember where Arti state goes. [`Node::serve`] starts the listener.
    pub fn listen_onion(&self, state_dir: impl Into<PathBuf>) {
        *self.onion_dir.lock().unwrap_or_else(|err| err.into_inner()) = Some(state_dir.into());
    }

    pub(crate) fn advertised_onion(&self) -> Option<String> {
        self.advertised_onion.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }

    /// Advertise `onion` as this node's letter address. The loopback shop URL
    /// is left alone. The file is how another process learns the address.
    pub(crate) fn publish_onion(&self, onion: &OnionAddress) -> Result<(), String> {
        let path = self.data_dir.join("onion.address");
        let tmp = self.data_dir.join("onion.address.tmp");
        fs::write(&tmp, format!("{onion}\n")).map_err(|err| err.to_string())?;
        fs::rename(&tmp, &path).map_err(|err| err.to_string())?;
        *self.advertised_onion.lock().unwrap_or_else(|err| err.into_inner()) = Some(onion.to_string());
        Ok(())
    }

    pub(crate) fn run_onion(&self, stop: &AtomicBool) {
        let state_dir = match self.onion_dir.lock().unwrap_or_else(|err| err.into_inner()).clone() {
            Some(dir) => dir,
            None => return,
        };
        let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("ob-node: onion listener: {err}");
                return;
            }
        };
        if let Err(err) = runtime.block_on(self.accept_onion(&state_dir, stop)) {
            eprintln!("ob-node: onion listener: {err}");
        }
    }

    async fn accept_onion(&self, state_dir: &Path, stop: &AtomicBool) -> Result<(), String> {
        let mut running = RunningMailbox::start(state_dir).map_err(|err| err.to_string())?;
        let shop = *self.onion_shop.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(gate) = shop {
            running.set_shop(gate);
        }
        let onion = running.onion_address().map_err(|err| err.to_string())?;
        self.publish_onion(&onion)?;
        eprintln!("ob-node onion {onion}");
        if shop.is_some() {
            eprintln!("ob-node: store at http://{onion}/ (public store, media, and reviews)");
        }
        eprintln!("ob-node: bootstrapping tor");
        running.bootstrap().await.map_err(|err| err.to_string())?;
        eprintln!("ob-node: tor bootstrapped; onion service {}", running.status_summary());
        let mut stop_wait = Box::pin(async {
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
        });
        let mut next_status = std::time::Instant::now();
        loop {
            tokio::select! {
                result = running.accept_letter(
                    &self.mailbox,
                    |kind, id| self.directory_reply(kind, &id),
                    |kind, body| self.directory_submit(kind, body),
                ) => {
                    match result {
                        Ok(tormail::Arrival::Letter(id)) => eprintln!("ob-node: stored letter {id}"),
                        Ok(tormail::Arrival::Shop) | Ok(tormail::Arrival::Query) => {}
                        Err(tormail::Error::ListenerClosed) => break,
                        Err(err) => {
                            eprintln!("ob-node: {err}");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
                _ = &mut stop_wait => break,
            }
            if next_status.elapsed() >= Duration::from_secs(20) {
                eprintln!("ob-node: onion service {}", running.status_summary());
                next_status = std::time::Instant::now();
            }
        }
        Ok(())
    }
}

/// `ob-node send-letter`.
pub struct SendLetter {
    pub peer: String,
    pub file: PathBuf,
    pub data_root: PathBuf,
    pub network: NetworkMode,
    pub timeout: Duration,
}

impl SendLetter {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut peer = None;
        let mut file = None;
        let mut data_root = PathBuf::from("data");
        let mut network = None;
        let mut timeout = 180u64;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| iter.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--to" => peer = Some(value("--to")?),
                "--file" => file = Some(PathBuf::from(value("--file")?)),
                "--data-dir" => data_root = PathBuf::from(value("--data-dir")?),
                "--network" => network = Some(NetworkMode::from_flag(&value("--network")?)?),
                "--timeout" => {
                    let text = value("--timeout")?;
                    timeout = text.parse::<u64>().ok().filter(|secs| *secs > 0).ok_or_else(|| {
                        "--timeout must be a positive number of seconds".to_string()
                    })?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(Self {
            peer: peer.ok_or_else(|| "--to needs an onion address or a loopback http:// URL".to_string())?,
            file: file.ok_or_else(|| "--file needs a sealed letter".to_string())?,
            data_root,
            network: network.unwrap_or(NetworkMode::Testnet4),
            timeout: Duration::from_secs(timeout),
        })
    }

    fn tor_dir(&self) -> PathBuf {
        self.network.data_dir(&self.data_root).join("tor")
    }

    /// Read the letter and deliver it. Prints one line on success.
    pub fn run(&self) -> Result<(), String> {
        let meta = fs::metadata(&self.file).map_err(|err| format!("{}: {err}", self.file.display()))?;
        if meta.len() > READ_LIMIT {
            return Err("that letter is too large".into());
        }
        let bytes = fs::read(&self.file).map_err(|err| format!("{}: {err}", self.file.display()))?;
        deliver_letter(&self.tor_dir(), &self.peer, &bytes, self.timeout)?;
        println!("sent {} bytes to {}", bytes.len(), self.peer.trim());
        Ok(())
    }
}

/// `ob-node poll-mail`.
pub struct PollMail {
    pub data_root: PathBuf,
    pub network: NetworkMode,
    pub expect: Option<String>,
}

impl PollMail {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut data_root = PathBuf::from("data");
        let mut network = None;
        let mut expect = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| iter.next().cloned().ok_or_else(|| format!("{name} needs a value"));
            match arg.as_str() {
                "--data-dir" => data_root = PathBuf::from(value("--data-dir")?),
                "--network" => network = Some(NetworkMode::from_flag(&value("--network")?)?),
                "--expect" => {
                    let text = value("--expect")?.to_ascii_lowercase();
                    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        return Err("--expect needs a sha256 hex digest".into());
                    }
                    expect = Some(text);
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(Self {
            data_root,
            network: network.unwrap_or(NetworkMode::Testnet4),
            expect,
        })
    }

    fn mailbox(&self) -> PathBuf {
        self.network.data_dir(&self.data_root).join("mailbox")
    }

    /// Print `id digest` lines. `Ok(false)` means `--expect` was not in the mailbox.
    pub fn run(&self) -> Result<bool, String> {
        let found = poll_mailbox(&self.mailbox())?;
        for letter in &found {
            println!("{} {}", letter.id, letter.sha256);
        }
        match &self.expect {
            None => Ok(true),
            Some(digest) => {
                if found.iter().any(|letter| letter.sha256.eq_ignore_ascii_case(digest)) {
                    Ok(true)
                } else {
                    eprintln!("letter {digest} is not in the mailbox");
                    Ok(false)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    use tormail::{seal_message, SenderKey, StoreKeypair};

    use super::*;
    use crate::harness::TempDir;
    use crate::http::dispatch;
    use crate::{load_catalog, NetworkMode, Node};

    const TEST_ONION: &str = "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";

    fn letter() -> Vec<u8> {
        seal_message(b"onion letter", StoreKeypair::generate().public(), &SenderKey::generate())
            .unwrap()
            .to_bytes()
    }

    struct Record {
        loopback: Mutex<Vec<String>>,
        onion: Mutex<Vec<String>>,
    }

    impl LetterSend for Record {
        fn loopback(&self, url: &str, _: &SealedLetter) -> Result<(), String> {
            self.loopback.lock().unwrap().push(url.to_string());
            Ok(())
        }

        fn onion(&self, onion: &OnionAddress, _: &SealedLetter) -> Result<(), String> {
            self.onion.lock().unwrap().push(onion.as_str().to_string());
            Ok(())
        }
    }

    #[test]
    fn an_onion_peer_uses_arti_and_not_loopback_http() {
        let sealed = SealedLetter::from_bytes(&letter()).unwrap();
        let record = Record {
            loopback: Mutex::new(Vec::new()),
            onion: Mutex::new(Vec::new()),
        };
        let mixed: String = TEST_ONION
            .chars()
            .enumerate()
            .map(|(index, ch)| if index % 2 == 0 { ch.to_ascii_uppercase() } else { ch })
            .collect();
        send_routed(&mixed, &sealed, &record).unwrap();
        assert_eq!(record.onion.lock().unwrap().as_slice(), [TEST_ONION.to_string()]);
        assert!(record.loopback.lock().unwrap().is_empty(), "an onion is not posted over HTTP");

        send_routed("http://127.0.0.1:9", &sealed, &record).unwrap();
        assert_eq!(record.loopback.lock().unwrap().as_slice(), ["http://127.0.0.1:9".to_string()]);
        assert_eq!(record.onion.lock().unwrap().len(), 1, "loopback does not dial Arti");

        let http_onion = format!("http://{TEST_ONION}");
        for peer in ["http://example.com", "http://203.0.113.5:8787", http_onion.as_str(), "not-an-onion", ""] {
            assert!(send_routed(peer, &sealed, &record).is_err(), "{peer}");
        }
        assert!(check_loopback(TEST_ONION).is_err());
        assert_eq!(record.onion.lock().unwrap().len(), 1);
        assert_eq!(record.loopback.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_advertised_address_is_the_onion_and_the_shop_stays_loopback() {
        let dir = TempDir::new();
        let node = Node::open(&dir.0, load_catalog(NetworkMode::Testnet4).unwrap()).unwrap();
        let raw = b"GET /network HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let response = dispatch(&node, raw);
        let body = std::str::from_utf8(&response).unwrap();
        let json: serde_json::Value = serde_json::from_str(body.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert!(json["letterOnion"].is_null());

        let onion = OnionAddress::parse(TEST_ONION).unwrap();
        node.publish_onion(&onion).unwrap();
        let text = fs::read_to_string(dir.0.join("onion.address")).unwrap();
        assert_eq!(text.trim(), TEST_ONION);
        let response = dispatch(&node, raw);
        let body = std::str::from_utf8(&response).unwrap();
        let json: serde_json::Value = serde_json::from_str(body.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(json["letterOnion"], TEST_ONION);
        assert!(node.escrow.config.mailbox_url.is_none(), "publishing an onion does not invent an HTTP peer");
    }

    #[test]
    fn loopback_delivery_posts_the_letter_and_poll_reads_the_mailbox() {
        let bytes = letter();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = bytes.clone();
        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            while !buf.windows(expected.len()).any(|window| window == expected) {
                match sock.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            assert!(buf.windows(expected.len()).any(|window| window == expected.as_slice()), "the POST body is the sealed letter");
            let head = String::from_utf8_lossy(&buf);
            assert!(head.contains("POST /escrow/mail"), "{head}");
            sock.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        deliver_letter(Path::new("unused"), &format!("http://127.0.0.1:{port}"), &bytes, Duration::from_secs(5)).unwrap();
        server.join().unwrap();

        let err = deliver_letter(Path::new("unused"), "http://198.51.100.1", &bytes, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("not a loopback"), "{err}");

        let dir = TempDir::new();
        let mailbox = tormail::Mailbox::open(&dir.0).unwrap();
        mailbox.accept(&SealedLetter::from_bytes(&bytes).unwrap()).unwrap();
        let found = poll_mailbox(&dir.0).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].sha256, letter_digest(&bytes));
        assert!(poll_mailbox(&dir.0.join("missing")).is_err());
    }

    #[test]
    fn send_and_poll_flags_parse() {
        let send = SendLetter::parse(&[
            "--to".into(),
            TEST_ONION.into(),
            "--file".into(),
            "letter.bin".into(),
            "--data-dir".into(),
            "/tmp/ob-buyer".into(),
            "--timeout".into(),
            "30".into(),
        ])
        .unwrap();
        assert_eq!(send.peer, TEST_ONION);
        assert_eq!(send.timeout, Duration::from_secs(30));
        assert_eq!(send.tor_dir(), PathBuf::from("/tmp/ob-buyer/testnet4/tor"));
        assert!(SendLetter::parse(&["--to".into(), TEST_ONION.into()]).is_err());
        assert!(SendLetter::parse(&["--timeout".into(), "0".into(), "--to".into(), "x".into(), "--file".into(), "y".into()]).is_err());

        let poll = PollMail::parse(&[
            "--data-dir".into(),
            "/tmp/ob-vendor".into(),
            "--expect".into(),
            "ab".repeat(32),
        ])
        .unwrap();
        assert_eq!(poll.mailbox(), PathBuf::from("/tmp/ob-vendor/testnet4/mailbox"));
        assert!(PollMail::parse(&["--expect".into(), "abcd".into()]).is_err());
    }

    #[test]
    fn the_sale_onion_serves_the_public_store_and_a_clearnet_host_is_not_dialed() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        use std::time::Instant;

        assert!(tormail::is_shop_preface(b"GET /public/store"));
        assert!(!tormail::is_shop_preface(b"OBML"), "a sealed letter is not a shop request");
        assert!(crate::sale_gate::allowed("GET", "/public/store"));
        assert!(!crate::sale_gate::allowed("GET", "/wallet"));
        assert!(!crate::sale_gate::allowed("POST", "/orders/abc/pay"));
        assert!(!crate::sale_gate::allowed("GET", "/mail"));
        assert!(!crate::sale_gate::allowed("GET", "/escrow/mail"));

        let dir = crate::harness::TempDir::new();
        let chain = crate::harness::MockChain::new();
        let node = crate::harness::open_node(&dir.0, NetworkMode::Testnet4, &chain, 1, crate::EscrowConfig::default());
        crate::seller_store_tests::open_store(&node);
        let (direct_status, direct_body) = crate::seller_store_tests::peer_request(&node, "/public/store");
        assert!(direct_status.contains("200"), "{direct_status}");

        let shop = TcpListener::bind("127.0.0.1:0").unwrap();
        let shop_port = shop.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let gate = crate::sale_gate::bind().unwrap();
        let gate_addr = gate.local_addr().unwrap();
        let trap = TcpListener::bind("127.0.0.1:0").unwrap();
        let trap_hits = Arc::new(AtomicUsize::new(0));

        thread::scope(|scope| {
            let record = Arc::clone(&seen);
            let stopping = Arc::clone(&stop);
            scope.spawn(move || serve_shop(shop, shop_port, &node, &record, &stopping));
            let stopping = Arc::clone(&stop);
            scope.spawn(move || crate::sale_gate::serve(gate, shop_port, &stopping));
            let counted = Arc::clone(&trap_hits);
            let stopping = Arc::clone(&stop);
            scope.spawn(move || {
                trap.set_nonblocking(true).ok();
                while !stopping.load(Ordering::Relaxed) {
                    match trap.accept() {
                        Ok((mut stream, _)) => {
                            counted.fetch_add(1, Ordering::SeqCst);
                            let mut buf = [0u8; 128];
                            let _ = stream.read(&mut buf);
                        }
                        Err(_) => thread::sleep(Duration::from_millis(20)),
                    }
                }
            });

            let store = talk(gate_addr, "GET /public/store HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n");
            assert!(store.starts_with("HTTP/1.1 200"), "{store}");
            let body = store.split("\r\n\r\n").nth(1).unwrap_or("");
            assert_eq!(body.as_bytes(), direct_body.as_slice(), "{body}");

            for closed in [
                "GET /wallet HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n",
                "POST /orders/abc/pay HTTP/1.1\r\nHost: example.onion\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "GET /mail HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n",
                "GET /escrow/mail HTTP/1.1\r\nHost: example.onion\r\nConnection: close\r\n\r\n",
            ] {
                let answer = talk(gate_addr, closed);
                assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
            }
            thread::sleep(Duration::from_millis(150));
            let forwarded = seen.lock().unwrap().clone();
            assert_eq!(forwarded.len(), 1, "a closed route was forwarded: {forwarded:?}");
            assert!(forwarded[0].starts_with("GET /public/store "), "{forwarded:?}");
            assert!(forwarded[0].contains(&format!("Host: 127.0.0.1:{shop_port}")), "{forwarded:?}");
            assert!(!forwarded[0].contains("example"), "{forwarded:?}");

            let started = Instant::now();
            for clearnet in ["http://example.com/public/store", "http://203.0.113.5:8787/public/store"] {
                let err = crate::peer_http::call(
                    &crate::peer_http::PeerLink::default(),
                    "GET",
                    clearnet,
                    None,
                    None,
                    Duration::from_secs(1),
                    1024,
                )
                .unwrap_err();
                assert!(started.elapsed() < Duration::from_millis(500), "a clearnet host was dialed: {err}");
                assert!(err.contains("not a loopback"), "{clearnet}: {err}");
            }
            thread::sleep(Duration::from_millis(100));
            assert_eq!(trap_hits.load(Ordering::SeqCst), 0, "a clearnet host was dialed");
            stop.store(true, Ordering::SeqCst);
        });
    }

    fn serve_shop(listener: TcpListener, port: u16, node: &Node, seen: &Mutex<Vec<String>>, stop: &AtomicBool) {
        listener.set_nonblocking(true).ok();
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let Ok(raw) = crate::http_parse::read(&mut stream) else {
                        continue;
                    };
                    seen.lock().unwrap().push(String::from_utf8_lossy(&raw).into_owned());
                    let response = crate::http::respond(node, &raw, crate::http::Client { loopback: true, port });
                    let _ = stream.write_all(&response);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(20)),
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    fn talk(addr: std::net::SocketAddr, request: &str) -> String {
        use std::net::TcpStream;
        let mut last = String::new();
        for _ in 0..25 {
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                stream.write_all(request.as_bytes()).unwrap();
                let mut buf = Vec::new();
                let _ = stream.read_to_end(&mut buf);
                if !buf.is_empty() {
                    return String::from_utf8_lossy(&buf).into_owned();
                }
            } else {
                last = "connection refused".into();
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the sale onion did not answer: {last}");
    }
}
