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

const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Present when the user chose "Run this computer as a home node".
const HOME_MODE_FILE: &str = "home-node-mode";

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

impl Sidecar {
    /// Start the node on `port` (the saved one, else a free one, when `None`).
    /// In home mode it also publishes its onion service and the LAN channel
    /// for paired devices. `file_vault` keeps the node's keys in a file in the
    /// data folder instead of the OS keychain (for a disposable profile).
    pub fn start(app_data_dir: &Path, port: Option<u16>, file_vault: bool, shop: Option<&Path>) -> Result<Self, String> {
        create_private_dir(app_data_dir)
            .map_err(|err| format!("Could not create {}: {err}", app_data_dir.display()))?;
        let data_dir = app_data_dir.join(NETWORK);
        create_private_dir(&data_dir)
            .map_err(|err| format!("Could not create {}: {err}", data_dir.display()))?;
        let log_path = data_dir.join("node.log");
        let log = open_private_log(&log_path)
            .map_err(|err| format!("Could not open {}: {err}", log_path.display()))?;
        let port = match port {
            Some(port) => port,
            None => node_port(&data_dir).map_err(|err| format!("No free loopback port: {err}"))?,
        };
        let binary = sidecar_path().map_err(|err| format!("Could not find ob-node: {err}"))?;

        let mut command = Command::new(&binary);
        command
            .arg("serve")
            .arg(port.to_string())
            .arg("--network")
            .arg(NETWORK)
            .arg("--data-dir")
            .arg(app_data_dir)
            .arg("--no-pair-codes")
            .args(if home_mode(app_data_dir) { &["--remote", "--lan"][..] } else { &[] })
            .arg("--onion")
            .current_dir(&data_dir)
            .env("OB_EXIT_ON_STDIN_EOF", "1")
            .stdin(Stdio::piped())
            .stdout(log.try_clone().map_err(|err| err.to_string())?)
            .stderr(log);
        if file_vault {
            command.arg("--vault").arg("file");
        }
        if let Some(shop) = shop {
            command.arg("--local-shop").arg(shop);
        }
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
        sidecar.wait_until_listening(port, &log_path)?;
        sidecar.token = read_token(&data_dir.join("pairing.token")).map_err(|err| {
            sidecar.stop();
            format!("Could not read the node's pairing token: {err}")
        })?;
        Ok(sidecar)
    }

    fn wait_until_listening(&mut self, port: u16, log_path: &Path) -> Result<(), String> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!(
                    "The OpenBazaar node stopped during startup ({status}). Details are in {}.",
                    log_path.display()
                ));
            }
            if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.stop();
                return Err(format!(
                    "The OpenBazaar node did not start in time. Details are in {}.",
                    log_path.display()
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
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
        Err(io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))
    }
}

/// The node writes the token before it listens, so it exists once the port answers.
fn read_token(path: &Path) -> io::Result<String> {
    let token = fs::read_to_string(path)?.trim().to_string();
    if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(token)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "not a pairing token"))
    }
}

/// The port saved in `node.port` if it is still free, else a new one, saved.
/// Other nodes keep this node's URL for mail and follows, so it must not
/// change between launches.
fn node_port(data_dir: &Path) -> io::Result<u16> {
    let path = data_dir.join("node.port");
    let saved = fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<u16>().ok());
    if let Some(port) = saved.filter(|&port| port != 0 && TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()) {
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
    stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|err| err.to_string())?;
    let request = format!(
        "POST /browser-session HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Ob-Pair: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    std::io::Write::write_all(&mut stream, request.as_bytes()).map_err(|err| err.to_string())?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut raw).map_err(|err| err.to_string())?;
    let text = String::from_utf8(raw).map_err(|_| "The shop sent a session the app could not read.".to_string())?;
    let body = text.split("\r\n\r\n").nth(1).ok_or_else(|| "The shop did not start a browser session.".to_string())?;
    accept_browser_url(origin, body, token).ok_or_else(|| "The shop did not start a browser session.".to_string())
}

pub(crate) fn accept_browser_url(origin: &str, body: &str, token: &str) -> Option<String> {
    let url = body.trim();
    let ticket = url.strip_prefix(&format!("{origin}/browser/"))?;
    if ticket.len() == 64 && ticket.bytes().all(|byte| byte.is_ascii_hexdigit()) && ticket != token {
        Some(url.to_string())
    } else {
        None
    }
}

fn loopback_port(origin: &str) -> Result<u16, String> {
    let rest = origin.strip_prefix("http://127.0.0.1:").ok_or_else(|| "The shop address is not on this machine.".to_string())?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("The shop address is not on this machine.".to_string());
    }
    rest.parse().map_err(|_| "The shop address is not on this machine.".to_string())
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
        assert_eq!(fs::read_to_string(dir.join("node.port")).unwrap(), first.to_string());
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
        assert_eq!(fs::read_to_string(dir.join("node.port")).unwrap(), port.to_string());
        fs::write(dir.join("node.port"), "not a port").unwrap();
        assert_ne!(node_port(&dir).unwrap(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_browser_link_is_a_one_time_local_path() {
        let origin = "http://127.0.0.1:56562";
        let token = "ab".repeat(32);
        let ticket = "cd".repeat(32);
        let url = accept_browser_url(origin, &format!("{origin}/browser/{ticket}\n"), &token).unwrap();
        assert_eq!(url, format!("{origin}/browser/{ticket}"));
        assert!(accept_browser_url(origin, &format!("{origin}/browser/{token}\n"), &token).is_none());
        assert!(accept_browser_url(origin, "http://evil.example/browser/cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n", &token).is_none());
        assert!(loopback_port("http://127.0.0.1:56562").is_ok());
        assert!(loopback_port("http://evil.example:56562").is_err());
    }
}
