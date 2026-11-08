//! Two node processes, one sealed order letter, over Tor.
//!
//! This is ignored so `cargo test --locked` stays offline. A dial failure, a
//! timeout, or a missing letter fails the test.
//!
//! ```text
//! cargo test --locked -p ob-node --test onion_letter -- --ignored --nocapture
//! ```
//!
//! The same steps by hand, with `letter.bin` a sealed tormail letter:
//!
//! ```text
//! ob-node serve 0 --vault file --data-dir /tmp/ob-vendor --no-rates --no-public-fallback --onion
//! ob-node send-letter --to "$(cat /tmp/ob-vendor/testnet4/onion.address)" --file letter.bin --data-dir /tmp/ob-buyer
//! ob-node poll-mail --data-dir /tmp/ob-vendor --expect "$(ob-node's sha256 of letter.bin)"
//! ```
//!
//! Success is send-letter exiting 0 and poll-mail exiting 0 with the letter
//! bytes stored under the vendor's `mailbox/`.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tormail::{seal_order, DropId, ListingHash, Order, PaymentRail, ReplyKeypair, ReplyPath, SenderKey, StoreKeypair};

const BUDGET: Duration = Duration::from_secs(360);

#[test]
#[ignore = "bootstraps two Arti clients; fails unless the sealed letter arrives"]
fn two_nodes_exchange_a_letter_over_tor() {
    let root = std::env::temp_dir().join(format!("ob-onion-{}-{}", std::process::id(), unique()));
    let vendor = root.join("vendor");
    let buyer = root.join("buyer");
    fs::create_dir_all(&buyer).unwrap();
    let _clean = Clean(root);

    let bytes = sealed_order();
    let letter_path = buyer.join("letter.bin");
    fs::write(&letter_path, &bytes).unwrap();
    let digest = ob_node::letter_digest(&bytes);

    let log_path = vendor.join("serve.log");
    let mut serve = Serve::spawn(&vendor, &log_path);
    let deadline = Instant::now() + BUDGET;
    let onion = wait_for_onion(&vendor.join("testnet4").join("onion.address"), &mut serve, &log_path, deadline);
    let onion = onion.trim().to_string();
    assert!(onion.ends_with(".onion"), "advertised {onion}");
    assert!(!onion.contains("127.0.0.1"), "the letter address is the onion, not loopback HTTP");

    let mut attempts = Vec::new();
    let mut sent = false;
    while Instant::now() + Duration::from_secs(5) < deadline {
        let left = deadline.saturating_duration_since(Instant::now()).as_secs().clamp(15, 60);
        let timeout = left.to_string();
        let output = Command::new(env!("CARGO_BIN_EXE_ob-node"))
            .args([
                "send-letter",
                "--to",
                &onion,
                "--file",
                letter_path.to_str().unwrap(),
                "--data-dir",
                buyer.to_str().unwrap(),
                "--timeout",
                &timeout,
            ])
            .output()
            .unwrap();
        attempts.push(format!(
            "status {}\nstdout {}\nstderr {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
        if output.status.success() {
            sent = true;
            break;
        }
        if serve.exited() {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let mailbox = vendor.join("testnet4").join("mailbox");
    let stored = letter_is_stored(&mailbox, &bytes);
    assert!(
        sent,
        "send-letter never delivered the letter (stored on disk: {stored}).\n{}\nvendor log:\n{}",
        attempts.iter().rev().take(3).cloned().collect::<Vec<_>>().join("\n---\n"),
        tail(&log_path)
    );

    let arrived = wait_until(Duration::from_secs(20), || letter_is_stored(&mailbox, &bytes));
    assert!(arrived, "the vendor mailbox does not contain the letter.\nvendor log:\n{}", tail(&log_path));

    let poll = Command::new(env!("CARGO_BIN_EXE_ob-node"))
        .args(["poll-mail", "--data-dir", vendor.to_str().unwrap(), "--expect", &digest])
        .output()
        .unwrap();
    assert!(
        poll.status.success(),
        "poll-mail did not see {digest}\nstdout {}\nstderr {}",
        String::from_utf8_lossy(&poll.stdout),
        String::from_utf8_lossy(&poll.stderr)
    );
}

fn sealed_order() -> Vec<u8> {
    let vendor = StoreKeypair::generate();
    let reply = ReplyKeypair::generate();
    let order = Order::new(
        ListingHash([9u8; 32]),
        1,
        BTreeMap::new(),
        "14 Harbor Lane",
        PaymentRail::OnChain,
        ReplyPath::sealed_only(*reply.public_key(), DropId::generate()),
    )
    .unwrap();
    seal_order(&order, vendor.public(), &SenderKey::generate()).unwrap().to_bytes()
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
            panic!("the vendor did not advertise an onion.\n{}", tail(log_path));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn letter_is_stored(mailbox: &Path, bytes: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(mailbox) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(".letter") {
            continue;
        }
        if fs::read(entry.path()).ok().as_deref() == Some(bytes) {
            return true;
        }
    }
    false
}

fn wait_until(limit: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if ready() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return format!("(no log at {})", path.display());
    };
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok();
    let mut lines: Vec<_> = buf.lines().collect();
    let skip = lines.len().saturating_sub(80);
    lines.drain(..skip);
    lines.join("\n")
}

fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

struct Serve {
    child: Child,
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
        Self { child }
    }

    fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Clean(PathBuf);

impl Drop for Clean {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
