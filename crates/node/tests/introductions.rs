//! Two introduction processes over Tor.
//!
//! One serves a higher seed-document sequence than the other. A peer keeps
//! the higher sequence. After the higher host is stopped, a new peer still
//! receives the document from the host that is left.
//!
//! Ignored so `cargo test` stays offline.
//!
//! ```text
//! cargo test -p ob-node --test introductions -- --ignored --nocapture
//! ```

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_secs(360);

#[test]
#[ignore = "bootstraps two introduction onions and fetches the seed document"]
fn one_stopped_introduction_still_returns_the_document_over_tor() {
    let key_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/seed-document.key");
    let key_text = fs::read_to_string(&key_path).unwrap_or_else(|err| panic!("operator key {}: {err}", key_path.display()));
    let key = unhex32(key_text.trim()).expect("operator key is 64 hex characters");

    let root = std::env::temp_dir().join(format!("ob-intro-{}-{}", std::process::id(), unique()));
    let _clean = Clean(root.clone());
    let low_root = root.join("low");
    let high_root = root.join("high");
    plant(&low_root, 2, key);
    plant(&high_root, 9, key);

    let low_log = low_root.join("serve.log");
    let high_log = high_root.join("serve.log");
    let mut low = Serve::spawn(&low_root, &low_log);
    let mut high = Serve::spawn(&high_root, &high_log);
    let deadline = Instant::now() + BUDGET;
    let low_onion = wait_for_onion(&low_root.join("testnet4").join("onion.address"), &mut low, &low_log, deadline);
    let high_onion = wait_for_onion(&high_root.join("testnet4").join("onion.address"), &mut high, &high_log, deadline);

    fetch_until(&root.join("peer-both"), &[&low_onion, &high_onion], 9, &low, &low_log, &high_log, deadline);
    high.stop();
    fetch_until(&root.join("peer-low"), &[&low_onion], 2, &low, &low_log, &high_log, Instant::now() + Duration::from_secs(180));
}

fn plant(data_root: &Path, sequence: u64, key: [u8; 32]) {
    let bytes = ob_node::sign_document(sequence, Vec::new(), key).unwrap();
    let dir = data_root.join("testnet4").join("directory");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("document.obsd"), bytes).unwrap();
}

fn fetch_until(data_root: &Path, onions: &[&str], want: u64, low: &Serve, low_log: &Path, high_log: &Path, deadline: Instant) {
    let mut last = String::new();
    while Instant::now() + Duration::from_secs(5) < deadline {
        if low.exited() {
            panic!("the low introduction exited\n{}", tail(low_log));
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_ob-node"));
        command.arg("fetch-seed").arg("--data-dir").arg(data_root);
        for onion in onions {
            command.arg("--from").arg(onion);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        last = format!("status {}\nstdout {stdout}\nstderr {stderr}", output.status);
        if output.status.success() && stdout.parse::<u64>().ok() == Some(want) {
            return;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    panic!("fetch-seed did not return sequence {want}\n{last}\nlow:\n{}\nhigh:\n{}", tail(low_log), tail(high_log));
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn wait_for_onion(path: &Path, serve: &mut Serve, log_path: &Path, deadline: Instant) -> String {
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            let text = text.trim().to_string();
            if text.ends_with(".onion") {
                return text;
            }
        }
        if serve.exited() || Instant::now() > deadline {
            panic!("an introduction did not advertise an onion.\n{}", tail(log_path));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return format!("(no log at {})", path.display());
    };
    let mut buf = String::new();
    let _ = file.read_to_string(&mut buf);
    let mut lines: Vec<_> = buf.lines().collect();
    let skip = lines.len().saturating_sub(40);
    lines.drain(..skip);
    lines.join("\n")
}

fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

struct Serve {
    child: Mutex<Child>,
}

impl Serve {
    fn spawn(data_dir: &Path, log_path: &Path) -> Self {
        fs::create_dir_all(data_dir).unwrap();
        let log = File::create(log_path).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_ob-node"))
            .args([
                "serve",
                "0",
                "--vault",
                "file",
                "--data-dir",
                data_dir.to_str().unwrap(),
                "--no-rates",
                "--no-public-fallback",
                "--onion",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        Self { child: Mutex::new(child) }
    }

    fn exited(&self) -> bool {
        matches!(self.child.lock().expect("serve").try_wait(), Ok(Some(_)))
    }

    fn stop(&self) {
        let mut child = self.child.lock().expect("serve");
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Clean(PathBuf);

impl Drop for Clean {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
