//! The bundled `ob-node` sidecar: one per app, on a free loopback port.
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

pub struct Sidecar {
    child: Child,
    pub origin: String,
    /// Read from `pairing.token` after the node starts; handed only to the app's own webview.
    pub token: String,
}

impl Sidecar {
    pub fn start(app_data_dir: &Path) -> Result<Self, String> {
        create_private_dir(app_data_dir)
            .map_err(|err| format!("Could not create {}: {err}", app_data_dir.display()))?;
        let data_dir = app_data_dir.join(NETWORK);
        create_private_dir(&data_dir)
            .map_err(|err| format!("Could not create {}: {err}", data_dir.display()))?;
        let log_path = data_dir.join("node.log");
        let log = open_private_log(&log_path)
            .map_err(|err| format!("Could not open {}: {err}", log_path.display()))?;
        let port = free_loopback_port().map_err(|err| format!("No free loopback port: {err}"))?;
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

/// The first boot prints the pairing token, so the log is user-only.
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

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}
