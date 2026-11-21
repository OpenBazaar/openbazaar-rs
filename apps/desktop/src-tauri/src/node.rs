//! The bundled `ob-node` sidecar: one per app, on a loopback port kept across launches.
//!
//! The node gets a piped stdin and exits when that pipe closes, so it stops
//! with the app even when the app is killed without running its exit hook.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Bitcoin network for these builds, passed to the node as `--network`. The
/// node keeps each network's keys in `<data dir>/<network>/`.
pub const NETWORK: &str = "testnet4";

/// Public Zcash testnet indexer from docs/zcash.md. Override with a
/// `zec-indexer` file in the app data folder (one URL).
const DEFAULT_ZEC_INDEXER: &str = "https://testnet.zec.rocks";
const ZEC_INDEXER_FILE: &str = "zec-indexer";
/// The node's own list of public Monero stagenet onion daemons. Override with
/// an `xmr-daemon` file in the app data folder (one line, comma-separated URLs).
const DEFAULT_XMR_DAEMONS: &str = "default";
const XMR_DAEMON_FILE: &str = "xmr-daemon";

/// How long the node may go without writing to its log, and without waiting
/// on the OS keychain, before startup counts as stuck. Longer than Tor's own
/// two-minute connect patience, so a slow Tor bootstrap is not cut short.
const STALL_TIMEOUT: Duration = Duration::from_secs(180);

/// Must match the node's lines around an OS keystore call (`vault_key.rs`).
/// Between them the node is blocked on a Keychain prompt the user has not
/// answered, which has no time limit.
const KEYSTORE_WAIT_LINE: &str = "ob-node: waiting for the OS keychain";
const KEYSTORE_DONE_LINE: &str = "ob-node: the OS keychain answered";

pub const KEYCHAIN_WAIT_MESSAGE: &str =
    "Waiting for macOS Keychain access. Click Always Allow in the system prompt.";

/// What the shell should do on one look at a starting node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Startup {
    Ready,
    Exited,
    Stalled,
    /// Blocked on an OS keychain prompt: wait without a limit.
    Keychain,
    Starting,
}

pub(crate) fn assess(exited: bool, listening: bool, log: &str, idle: Duration, stall: Duration) -> Startup {
    if exited {
        Startup::Exited
    } else if listening {
        Startup::Ready
    } else if waiting_on_keychain(log) {
        Startup::Keychain
    } else if idle >= stall {
        Startup::Stalled
    } else {
        Startup::Starting
    }
}

fn waiting_on_keychain(log: &str) -> bool {
    let wait = log.rfind(KEYSTORE_WAIT_LINE);
    wait.is_some() && wait > log.rfind(KEYSTORE_DONE_LINE)
}

/// Present when the user chose "Run this computer as a home node".
const HOME_MODE_FILE: &str = "home-node-mode";

/// Present when the user agreed to keep the vault key in a file because this
/// system has no OS keyring service (common on Qubes and minimal Linux).
const FILE_VAULT_FILE: &str = "file-vault";

/// How the node says the OS keyring cannot be reached (`vault_key.rs`). The
/// shell offers the file vault only on this error: a different vault error,
/// such as a missing entry, is not fixed by a new key file.
pub const KEYSTORE_UNAVAILABLE: &str = "The OS keystore is not available";

pub struct Sidecar {
    child: Child,
    pub port: u16,
    pub origin: String,
    /// Read from `pairing.token` after the node starts; handed only to the app's own webview.
    pub token: String,
}

pub fn home_mode(app_data_dir: &Path) -> bool {
    app_data_dir.join(HOME_MODE_FILE).is_file()
}

pub fn set_home_mode(app_data_dir: &Path, on: bool) -> io::Result<()> {
    let path = app_data_dir.join(HOME_MODE_FILE);
    if on {
        fs::write(path, b"")
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

pub fn file_vault_chosen(app_data_dir: &Path) -> bool {
    app_data_dir.join(FILE_VAULT_FILE).is_file()
}

/// Remember that the vault key lives in a file in the data folder, so every
/// later launch passes `--vault file` without asking again.
pub fn choose_file_vault(app_data_dir: &Path) -> io::Result<()> {
    fs::write(app_data_dir.join(FILE_VAULT_FILE), b"")
}

impl Sidecar {
    /// Start the node on `port` (the saved one, else a free one, when `None`).
    /// Always `--remote` so order and escrow mail get a store onion. Home mode
    /// also opens the LAN channel for paired devices. `file_vault` keeps the
    /// node's keys in a file in the data folder instead of the OS keychain
    /// (for a disposable profile); a choice saved with [`choose_file_vault`]
    /// does the same on every later launch.
    /// `on_keychain` is called once each time the node starts waiting on a
    /// Keychain prompt, so the user can be told what to click.
    pub fn start(
        app_data_dir: &Path,
        port: Option<u16>,
        file_vault: bool,
        shop: Option<&Path>,
        on_keychain: &dyn Fn(),
    ) -> Result<Self, String> {
        create_private_dir(app_data_dir)
            .map_err(|err| format!("Could not create {}: {err}", app_data_dir.display()))?;
        let data_dir = app_data_dir.join(NETWORK);
        create_private_dir(&data_dir)
            .map_err(|err| format!("Could not create {}: {err}", data_dir.display()))?;
        let log_path = data_dir.join("node.log");
        let log = open_private_log(&log_path)
            .map_err(|err| format!("Could not open {}: {err}", log_path.display()))?;
        // The log is appended across launches; only what this launch writes
        // past here belongs in this launch's error message.
        let log_from = fs::metadata(&log_path).map(|meta| meta.len()).unwrap_or(0);
        let port = match port {
            Some(port) => port,
            None => node_port(&data_dir).map_err(|err| format!("No free loopback port: {err}"))?,
        };
        let binary = sidecar_path().map_err(|err| format!("Could not find ob-node: {err}"))?;

        let mut command = Command::new(&binary);
        command
            .args(sidecar_args(port, app_data_dir, file_vault, shop))
            .current_dir(&data_dir)
            .env("OB_EXIT_ON_STDIN_EOF", "1")
            .stdin(Stdio::piped())
            .stdout(log.try_clone().map_err(|err| err.to_string())?)
            .stderr(log);
        hide_console(&mut command);
        let child = command
            .spawn()
            .map_err(|err| format!("Could not start {}: {err}", binary.display()))?;

        let mut sidecar = Self {
            child,
            port,
            origin: format!("http://127.0.0.1:{port}"),
            token: String::new(),
        };
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        wait_for_node(
            &mut sidecar.child,
            &log_path,
            log_from,
            STALL_TIMEOUT,
            || TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok(),
            on_keychain,
        )?;
        sidecar.token = read_token(&data_dir.join("pairing.token")).map_err(|err| {
            sidecar.stop();
            format!("Could not read the node's pairing token: {err}")
        })?;
        Ok(sidecar)
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wait until `listening` says the node is up. The node is killed only when it
/// made no progress (log growth) for `stall` while not on a Keychain prompt.
fn wait_for_node(
    child: &mut Child,
    log_path: &Path,
    log_from: u64,
    stall: Duration,
    mut listening: impl FnMut() -> bool,
    on_keychain: &dyn Fn(),
) -> Result<(), String> {
    let mut log_len = log_from;
    let mut last_progress = Instant::now();
    let mut was_on_keychain = false;
    loop {
        let status = child.try_wait().ok().flatten();
        let len = fs::metadata(log_path).map(|meta| meta.len()).unwrap_or(log_len);
        if len != log_len {
            log_len = len;
            last_progress = Instant::now();
        }
        let log = launch_log(log_path, log_from).unwrap_or_default();
        let ready = status.is_none() && listening();
        match assess(status.is_some(), ready, &log, last_progress.elapsed(), stall) {
            Startup::Ready => return Ok(()),
            Startup::Exited => {
                let status = status.expect("exited");
                return Err(match launch_error(log_path, log_from) {
                    Some(error) => format!("The OpenBazaar node stopped during startup ({status}): {error}"),
                    None => format!(
                        "The OpenBazaar node stopped during startup ({status}). Details are in {}.",
                        log_path.display()
                    ),
                });
            }
            Startup::Stalled => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match launch_error(log_path, log_from) {
                    Some(last) => format!(
                        "The OpenBazaar node stopped making progress during startup after \"{last}\". Details are in {}.",
                        log_path.display()
                    ),
                    None => format!(
                        "The OpenBazaar node stopped making progress during startup. Details are in {}.",
                        log_path.display()
                    ),
                });
            }
            Startup::Keychain => {
                if !was_on_keychain {
                    on_keychain();
                }
                was_on_keychain = true;
                last_progress = Instant::now();
            }
            Startup::Starting => was_on_keychain = false,
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The last 64 KiB this launch wrote to the log.
fn launch_log(log_path: &Path, from: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(log_path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(from.max(len.saturating_sub(64 * 1024)))).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// Arguments after the binary: `serve`, the loopback port, and the flags this
/// packaged testnet app always wants. `--tor` runs Tor inside the node on a
/// free loopback port, so nothing needs to be installed and a Tor already on
/// 9050 is left alone. `zec-indexer` and `xmr-daemon` files in the app data
/// folder override the Zcash and Monero defaults. Lightning runs on Mutinynet
/// signet through the node's default LSP, over the same Tor.
fn sidecar_args(
    port: u16,
    app_data_dir: &Path,
    file_vault: bool,
    shop: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        "serve".into(),
        port.to_string(),
        "--network".into(),
        NETWORK.into(),
        "--data-dir".into(),
        app_data_dir.display().to_string(),
        "--no-pair-codes".into(),
    ];
    args.push("--remote".into());
    if home_mode(app_data_dir) {
        args.push("--lan".into());
    }
    args.extend(["--onion".into(), "--tor".into()]);
    if file_vault || file_vault_chosen(app_data_dir) {
        args.extend(["--vault".into(), "file".into()]);
    }
    if let Some(shop) = shop {
        args.extend(["--local-shop".into(), shop.display().to_string()]);
    }
    args.extend([
        "--zec-indexer".into(),
        setting_line(app_data_dir, ZEC_INDEXER_FILE).unwrap_or_else(|| DEFAULT_ZEC_INDEXER.into()),
    ]);
    args.extend([
        "--xmr-daemon".into(),
        setting_line(app_data_dir, XMR_DAEMON_FILE).unwrap_or_else(|| DEFAULT_XMR_DAEMONS.into()),
        "--lightning".into(),
    ]);
    args
}

/// What the node last said, from the part of the log this launch wrote: its
/// last `ob-node:` line when there is one, else its last line. One bounded
/// line, so a startup dialog can carry the reason a node died or the step a
/// stalled one never finished.
fn launch_error(log_path: &Path, from: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(log_path).ok()?;
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut raw = Vec::new();
    file.take(64 * 1024).read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let mut last = None;
    let mut last_own = None;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(rest) = line.strip_prefix("ob-node: ") {
            last_own = Some(rest);
        }
        last = Some(line);
    }
    let mut line = last_own.or(last)?.to_string();
    if line.len() > 400 {
        let mut end = 400;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    Some(line)
}

/// First non-empty, non-comment line of `name` in `dir`.
fn setting_line(dir: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(name)).ok()?;
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
}

/// Tauri copies `externalBin` next to the app executable without the
/// target-triple suffix (Contents/MacOS on macOS, /usr/bin on Linux packages).
fn sidecar_path() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "app executable has no parent"))?;
    let path = dir.join(format!("ob-node{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            path.display().to_string(),
        ))
    }
}

/// The node writes the token before it listens, so it exists once the port answers.
fn read_token(path: &Path) -> io::Result<String> {
    let token = fs::read_to_string(path)?.trim().to_string();
    if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(token)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a pairing token",
        ))
    }
}

/// The port saved in `node.port` if it is still free, else a new one, saved.
/// Other nodes keep this node's URL for mail and follows, so it must not
/// change between launches.
fn node_port(data_dir: &Path) -> io::Result<u16> {
    let path = data_dir.join("node.port");
    let saved = fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok());
    if let Some(port) =
        saved.filter(|&port| port != 0 && TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok())
    {
        return Ok(port);
    }
    let port = free_loopback_port()?;
    fs::write(&path, port.to_string())?;
    Ok(port)
}

/// The OS picks the port; the listener is dropped right before the node binds it.
fn free_loopback_port() -> io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The node's log is user-only.
fn open_private_log(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The Expo web export, when a dev build can see `apps/mobile/dist`.
/// The packaged app looks in its resource directory before this.
pub fn shop_directory() -> Option<PathBuf> {
    let mut dir = std::env::current_exe().ok()?;
    for _ in 0..8 {
        if !dir.pop() {
            break;
        }
        let candidate = dir.join("apps/mobile/dist");
        if candidate.join("index.html").is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Ask the node for a one-time localhost path. The pairing token is sent only
/// to 127.0.0.1 and is never part of the URL that comes back.
pub fn browser_url(origin: &str, token: &str) -> Result<String, String> {
    let port = loopback_port(origin)?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_secs(2),
    )
    .map_err(|_| "The shop is not running on this machine.".to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|err| err.to_string())?;
    let request = format!(
        "POST /browser-session HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Ob-Pair: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    std::io::Write::write_all(&mut stream, request.as_bytes()).map_err(|err| err.to_string())?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut raw).map_err(|err| err.to_string())?;
    let text = String::from_utf8(raw)
        .map_err(|_| "The shop sent a session the app could not read.".to_string())?;
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .ok_or_else(|| "The shop did not start a browser session.".to_string())?;
    accept_browser_url(origin, body, token)
        .ok_or_else(|| "The shop did not start a browser session.".to_string())
}

pub(crate) fn accept_browser_url(origin: &str, body: &str, token: &str) -> Option<String> {
    let url = body.trim();
    let ticket = url.strip_prefix(&format!("{origin}/browser/"))?;
    if ticket.len() == 64 && ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) && ticket != token
    {
        Some(url.to_string())
    } else {
        None
    }
}

fn loopback_port(origin: &str) -> Result<u16, String> {
    let rest = origin
        .strip_prefix("http://127.0.0.1:")
        .ok_or_else(|| "The shop address is not on this machine.".to_string())?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("The shop address is not on this machine.".to_string());
    }
    rest.parse()
        .map_err(|_| "The shop address is not on this machine.".to_string())
}

/// The node's `--local-shop` page, which the desktop window and a browser on
/// this machine both load. Not the bundled Tauri origin: that page has no
/// `ob-local-shop` marker and cannot Continue without a pairing code.
pub fn shop_page_url(origin: &str) -> Result<String, String> {
    let _port = loopback_port(origin)?;
    Ok(format!("{origin}/"))
}

pub fn open_url(url: &str) -> Result<(), String> {
    browser_command()
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Could not open the browser: {err}"))
}

#[cfg(target_os = "macos")]
fn browser_command() -> Command {
    Command::new("open")
}

#[cfg(target_os = "windows")]
fn browser_command() -> Command {
    let mut command = Command::new("cmd");
    command.arg("/C").arg("start").arg("");
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_command() -> Command {
    Command::new("xdg-open")
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ob-desktop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_node_keeps_its_port_across_launches() {
        let dir = temp_dir("port-kept");
        let first = node_port(&dir).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("node.port")).unwrap(),
            first.to_string()
        );
        assert_eq!(node_port(&dir).unwrap(), first);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_taken_or_unreadable_port_is_replaced() {
        let dir = temp_dir("port-taken");
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let busy = taken.local_addr().unwrap().port();
        fs::write(dir.join("node.port"), busy.to_string()).unwrap();
        let port = node_port(&dir).unwrap();
        assert_ne!(port, busy);
        assert_eq!(
            fs::read_to_string(dir.join("node.port")).unwrap(),
            port.to_string()
        );
        fs::write(dir.join("node.port"), "not a port").unwrap();
        assert_ne!(node_port(&dir).unwrap(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_browser_link_is_a_one_time_local_path() {
        let origin = "http://127.0.0.1:56562";
        let token = "ab".repeat(32);
        let ticket = "cd".repeat(32);
        let url =
            accept_browser_url(origin, &format!("{origin}/browser/{ticket}\n"), &token).unwrap();
        assert_eq!(url, format!("{origin}/browser/{ticket}"));
        assert!(
            accept_browser_url(origin, &format!("{origin}/browser/{token}\n"), &token).is_none()
        );
        assert!(accept_browser_url(origin, "http://evil.example/browser/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n", &token).is_none());
        assert!(loopback_port("http://127.0.0.1:56562").is_ok());
        assert!(loopback_port("http://evil.example:56562").is_err());
        assert_eq!(shop_page_url(origin).unwrap(), "http://127.0.0.1:56562/");
        assert!(shop_page_url("https://tauri.localhost").is_err());
        assert!(shop_page_url("http://localhost:56562").is_err());
    }

    fn has_flag(args: &[String], name: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == name && pair[1] == value)
    }

    #[test]
    fn the_sidecar_runs_built_in_tor_with_zcash_monero_and_lightning_on() {
        let dir = temp_dir("sidecar-args");
        let args = sidecar_args(18787, &dir, false, None);
        assert!(args.iter().any(|arg| arg == "--tor"), "{args:?}");
        assert!(
            !args.iter().any(|arg| arg == "--proxy"),
            "no system Tor is needed or used: {args:?}"
        );
        assert!(
            has_flag(&args, "--zec-indexer", DEFAULT_ZEC_INDEXER),
            "{args:?}"
        );
        assert!(has_flag(&args, "--xmr-daemon", "default"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "--lightning"), "{args:?}");
        assert!(
            args.iter().any(|arg| arg == "--remote"),
            "escrow and order mail use the store onion: {args:?}"
        );
        assert!(!args.iter().any(|arg| arg == "--lan"), "{args:?}");
        fs::write(
            dir.join(ZEC_INDEXER_FILE),
            "# local Zaino\nhttp://127.0.0.1:8137\n",
        )
        .unwrap();
        fs::write(dir.join(XMR_DAEMON_FILE), "http://127.0.0.1:38081\n").unwrap();
        let args = sidecar_args(18787, &dir, true, None);
        assert!(
            has_flag(&args, "--zec-indexer", "http://127.0.0.1:8137"),
            "{args:?}"
        );
        assert!(
            has_flag(&args, "--xmr-daemon", "http://127.0.0.1:38081"),
            "{args:?}"
        );
        assert!(has_flag(&args, "--vault", "file"), "{args:?}");
        set_home_mode(&dir, true).unwrap();
        let args = sidecar_args(18787, &dir, false, None);
        assert!(args.iter().any(|arg| arg == "--remote"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "--lan"), "{args:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_chosen_file_vault_is_remembered_across_launches() {
        let dir = temp_dir("file-vault");
        let args = sidecar_args(18787, &dir, false, None);
        assert!(!args.iter().any(|arg| arg == "--vault"), "{args:?}");
        assert!(!file_vault_chosen(&dir));
        choose_file_vault(&dir).unwrap();
        assert!(file_vault_chosen(&dir));
        let args = sidecar_args(18787, &dir, false, None);
        assert!(has_flag(&args, "--vault", "file"), "{args:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_waits_on_the_keychain_and_fails_only_on_exit_or_stall() {
        let stall = Duration::from_secs(180);
        let quiet = Duration::from_secs(600);
        let on_prompt = format!("ob-node: built-in Tor connected\n{KEYSTORE_WAIT_LINE}\n");
        let answered = format!("{on_prompt}{KEYSTORE_DONE_LINE}\n");
        assert_eq!(assess(false, false, "", Duration::ZERO, stall), Startup::Starting);
        assert_eq!(assess(false, false, &on_prompt, quiet, stall), Startup::Keychain, "an unanswered prompt never times out");
        assert_eq!(assess(false, false, &answered, quiet, stall), Startup::Stalled);
        assert_eq!(assess(false, false, &answered, Duration::from_secs(5), stall), Startup::Starting);
        let asked_again = format!("{answered}{KEYSTORE_WAIT_LINE}\n");
        assert_eq!(assess(false, false, &asked_again, quiet, stall), Startup::Keychain);
        assert_eq!(assess(true, false, &on_prompt, Duration::ZERO, stall), Startup::Exited);
        assert_eq!(assess(false, true, &on_prompt, quiet, stall), Startup::Ready);
    }

    #[cfg(unix)]
    fn fake_node(dir: &Path, script: &str) -> (Child, PathBuf) {
        let log_path = dir.join("node.log");
        let log = open_private_log(&log_path).unwrap();
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        (child, log_path)
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_keychain_prompt_outlasts_the_stall_limit() {
        use std::cell::Cell;
        let dir = temp_dir("slow-vault");
        let script = format!(
            "echo '{KEYSTORE_WAIT_LINE}' >&2; sleep 3; echo '{KEYSTORE_DONE_LINE}' >&2; echo listening; sleep 30"
        );
        let (mut child, log_path) = fake_node(&dir, &script);
        let prompts = Cell::new(0);
        let started = Instant::now();
        let result = wait_for_node(
            &mut child,
            &log_path,
            0,
            Duration::from_secs(1),
            || fs::read_to_string(&log_path).unwrap_or_default().contains("listening"),
            &|| prompts.set(prompts.get() + 1),
        );
        assert_eq!(result, Ok(()));
        assert!(started.elapsed() >= Duration::from_secs(3));
        assert_eq!(prompts.get(), 1, "the user is told once per prompt");
        let _ = child.kill();
        let _ = child.wait();
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_silent_node_is_stopped_and_an_exited_one_reported() {
        let dir = temp_dir("stalled-node");
        let (mut child, log_path) = fake_node(&dir, "echo starting; sleep 30");
        let error = wait_for_node(&mut child, &log_path, 0, Duration::from_secs(1), || false, &|| {}).unwrap_err();
        assert!(error.contains("stopped making progress"), "{error}");
        assert!(error.contains("after \"starting\""), "the stall names the node's last step: {error}");
        assert!(child.try_wait().unwrap().is_some(), "a stuck node is killed");
        let (mut child, log_path) = fake_node(&dir, "echo 'ob-node: bad flag' >&2; exit 2");
        let error = wait_for_node(&mut child, &log_path, 0, Duration::from_secs(60), || false, &|| {}).unwrap_err();
        assert!(error.contains("stopped during startup") && error.ends_with("bad flag"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_startup_error_is_the_nodes_own_last_line_of_this_launch() {
        let dir = temp_dir("launch-error");
        let log = dir.join("node.log");
        fs::write(&log, "ob-node: an old launch failed\n").unwrap();
        let from = fs::metadata(&log).unwrap().len();
        assert_eq!(launch_error(&log, from), None, "an earlier launch's error is not this one's");
        let mut text = fs::read_to_string(&log).unwrap();
        text.push_str("some progress line\nob-node: The OS keystore is not available (no Secret Service). Use --vault passphrase, or --vault file on a headless machine.\n");
        fs::write(&log, &text).unwrap();
        let error = launch_error(&log, from).unwrap();
        assert!(error.starts_with(KEYSTORE_UNAVAILABLE), "{error}");
        fs::write(&log, "noise\npanicked at src/main.rs\n").unwrap();
        assert_eq!(launch_error(&log, 0).unwrap(), "panicked at src/main.rs", "without an ob-node line the last line is used");
        fs::remove_dir_all(dir).unwrap();
    }
}
