//! Real `ob-node serve` processes for tests.
//!
//! Each node runs the binary on a free loopback port with its own temporary
//! data dir, a file vault, no rates, no introductions, and the loopback chain
//! server the test names: one that only answers 503 ([`chain_stub`]), or a
//! stand-in Esplora ([`esplora`]). Nothing here reaches the internet unless a
//! test passes `--remote`. Dropping a node kills the process and removes its
//! data dir, also while a test is failing; the node's log is printed then.

pub mod esplora;

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A stand-in Esplora server that answers every request 503, so a node's
/// chain calls fail at once and on this machine. Returns its URL.
pub fn chain_stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.read(&mut [0u8; 1024]);
            let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    url
}

/// Run one step and say on stderr how long it took.
pub fn timed<T>(step: &str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let done = run();
    eprintln!("{step}: {:.1} s", started.elapsed().as_secs_f32());
    done
}

/// Poll `ready` until it answers `Ok`, for at most `limit`. A timeout panics
/// with `step` and the last `Err`.
pub fn wait_for<T>(step: &str, limit: Duration, mut ready: impl FnMut() -> Result<T, String>) -> T {
    let started = Instant::now();
    timed(step, || loop {
        match ready() {
            Ok(found) => return found,
            Err(last) if started.elapsed() > limit => panic!("timed out after {} s waiting for {step}. Last seen: {last}", limit.as_secs()),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    })
}

pub struct RealNode {
    pub name: &'static str,
    pub url: String,
    child: Child,
    token: String,
    /// The longest one request may take.
    patience: Duration,
    dir: tempfile::TempDir,
}

impl RealNode {
    /// Start `ob-node serve` with `extra` flags and wait until it answers.
    /// A moderator is `&["--moderate"]`; its seller takes `&["--moderator", url]`.
    pub fn start(name: &'static str, chain: &str, extra: &[&str], patience: Duration) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The node names this port in its reply address, so it cannot be 0.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let log = File::create(dir.path().join("serve.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ob-node"))
            .args(["serve", &port.to_string(), "--network", "testnet4", "--data-dir"])
            .arg(dir.path())
            // Without `--no-introductions` a node dials the network's
            // introduction onions over Tor, with or without `--remote`.
            .args(["--vault", "file", "--no-rates", "--no-introductions", "--esplora", chain, "--poll-secs", "1"])
            .args(extra)
            // The node exits when this pipe closes, so it does not outlive a test process that was killed.
            .env("OB_EXIT_ON_STDIN_EOF", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut node = Self { name, url: format!("http://127.0.0.1:{port}"), child, token: String::new(), patience, dir };
        wait_for(&format!("the {name} node to answer"), Duration::from_secs(30), || {
            if let Ok(Some(status)) = node.child.try_wait() {
                panic!("the {name} node exited: {status}");
            }
            node.send("GET", "/network", None, &[]).map(drop)
        });
        // The disposable node's own token, from its own data dir. It is never printed.
        node.token = fs::read_to_string(node.data_dir().join("pairing.token")).unwrap().trim().to_string();
        node
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.path().join("testnet4")
    }

    /// One request with a JSON body, as the node's own paired client.
    /// Returns the status and the JSON answer.
    pub fn request(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        self.send(method, path, Some("application/json"), body.as_bytes()).unwrap_or_else(|err| panic!("{} {method} {path}: {err}", self.name))
    }

    /// `POST /media` with raw image bytes.
    pub fn upload(&self, bytes: &[u8]) -> (u16, Value) {
        self.send("POST", "/media", Some("image/png"), bytes).unwrap_or_else(|err| panic!("{} POST /media: {err}", self.name))
    }

    /// An unpaired `GET`, as another node sends one.
    pub fn peer_get(&self, path: &str) -> (u16, Value) {
        self.send("GET", path, None, &[]).unwrap_or_else(|err| panic!("{} GET {path}: {err}", self.name))
    }

    /// `content_type` is set on a paired request; `None` sends no token.
    fn send(&self, method: &str, path: &str, content_type: Option<&str>, body: &[u8]) -> Result<(u16, Value), String> {
        let mut request = ureq::request(method, &format!("{}{path}", self.url)).timeout(self.patience);
        if let Some(content_type) = content_type {
            request = request.set("X-Ob-Pair", &self.token).set("Content-Type", content_type);
        }
        let sent = if body.is_empty() { request.call() } else { request.send_bytes(body) };
        let response = match sent {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(err) => return Err(err.to_string()),
        };
        let status = response.status();
        let text = response.into_string().map_err(|err| err.to_string())?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

impl Drop for RealNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let log = fs::read_to_string(self.dir.path().join("serve.log")).unwrap_or_default();
            // Every poll logs the chain stub's refusal, or the wallet scan the stand-in Esplora does not serve. Those lines are left out.
            let lines: Vec<&str> = log.lines().filter(|line| !line.contains(" 503") && !line.contains("wallet sync: ")).collect();
            eprintln!("--- {} node log ---\n{}", self.name, lines[lines.len().saturating_sub(60)..].join("\n"));
        }
    }
}
