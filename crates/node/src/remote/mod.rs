//! Remote access to a home node from paired devices. See `docs/home-node.md`.
//!
//! As a server (`ob-node serve --remote`), the node publishes an onion
//! service for its admin API and pipes it to a loopback [`gateway`]. A
//! device pairs once with a one-time code from the node's console or from an
//! already paired device ([`offers`]), registering its own key
//! ([`devices`]); from then on it signs every request ([`auth`]). There is
//! no account and no relay: the onion key authenticates the node to the
//! device, and the device key authenticates the device to the node.
//!
//! As a client, a node (the desktop app's sidecar) holds one device key for
//! one home node and forwards `/home/proxy/*` to it over Tor ([`home`]), so
//! the same shop window can manage a node elsewhere.

mod auth;
mod devices;
mod gateway;
mod home;
mod local_http;
mod offers;
mod pair_cli;
mod policy;
mod tor;
mod web;

#[cfg(test)]
mod tests;

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::vault::Vault;
use crate::{Error, Node};

pub use pair_cli::pair_command;
pub(crate) use home::{is_proxy_request, response as home_response};
pub(crate) use local_http::response as local_response;

/// Set by the gateway on requests it forwards; names the signing device.
pub(crate) const REMOTE_DEVICE_HEADER: &str = "X-Ob-Remote-Device";
const ONION_NICKNAME: &str = "ob-home";

/// `serve` flags for remote access, taken out of the argument list before
/// the other flags are parsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteConfig {
    /// `--remote`: publish the admin onion service.
    pub onion: bool,
    /// `--remote-listen 127.0.0.1:PORT`: the gateway's own address. Without
    /// `--remote` this is a loopback-only gateway for tests.
    pub listen: Option<SocketAddr>,
    /// `--web-dir DIR`: the web shop export served to remote browsers.
    pub web_dir: Option<PathBuf>,
    /// `--allow-loopback-home`: as a client, accept a pairing code that names
    /// a loopback address instead of an onion. Tests only.
    pub loopback_homes: bool,
}

impl RemoteConfig {
    /// Split remote flags from `args`; the rest go to `ServeOptions::parse`.
    pub fn extract(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut config = Self::default();
        let mut rest = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--remote" => config.onion = true,
                "--allow-loopback-home" => config.loopback_homes = true,
                "--remote-listen" => {
                    let text = iter.next().ok_or("--remote-listen needs an address")?;
                    let addr: SocketAddr = text.parse().map_err(|_| format!("--remote-listen must look like 127.0.0.1:8790, not {text:?}"))?;
                    if !addr.ip().is_loopback() {
                        return Err("--remote-listen must be a loopback address. Remote devices reach the node over its onion service; the gateway speaks plain HTTP and is never put on a network.".into());
                    }
                    config.listen = Some(addr);
                }
                "--web-dir" => config.web_dir = Some(PathBuf::from(iter.next().ok_or("--web-dir needs a directory")?)),
                _ => rest.push(arg.clone()),
            }
        }
        Ok((config, rest))
    }

    fn serves(&self) -> bool {
        self.onion || self.listen.is_some()
    }
}

pub(crate) struct Remote {
    devices: Mutex<devices::Devices>,
    offers: Mutex<offers::Offers>,
    pub(crate) replay: auth::ReplayGuard,
    config: OnceLock<RemoteConfig>,
    /// Where devices dial, once the gateway is up: the onion address, or
    /// the gateway's loopback address when there is no onion.
    address: OnceLock<String>,
    gateway: OnceLock<SocketAddr>,
    onion: OnceLock<tormail::OnionForward>,
    tor: OnceLock<Result<tor::Tor, String>>,
    home: home::HomeState,
}

impl Remote {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        Ok(Self {
            devices: Mutex::new(devices::Devices::open(dir, vault.clone())?),
            offers: Mutex::new(offers::Offers::default()),
            replay: auth::ReplayGuard::new(now_secs()),
            config: OnceLock::new(),
            address: OnceLock::new(),
            gateway: OnceLock::new(),
            onion: OnceLock::new(),
            tor: OnceLock::new(),
            home: home::HomeState::open(dir, vault)?,
        })
    }

    fn config(&self) -> RemoteConfig {
        self.config.get().cloned().unwrap_or_default()
    }

    pub(crate) fn devices(&self) -> MutexGuard<'_, devices::Devices> {
        self.devices.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn offers(&self) -> MutexGuard<'_, offers::Offers> {
        self.offers.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn node_id(&self) -> String {
        self.devices().node_id.clone()
    }

    pub(crate) fn web_dir(&self) -> Option<&Path> {
        self.config.get().and_then(|config| config.web_dir.as_deref())
    }

    fn serves_host(&self, host: &str) -> bool {
        let onion = self.onion.get().and_then(|onion| onion.onion_address()).map(|onion| onion.to_string());
        if onion.as_deref() == Some(host.strip_suffix(":80").unwrap_or(host)) {
            return true;
        }
        self.gateway.get().is_some_and(|addr| {
            host == addr.to_string() || host == format!("localhost:{}", addr.port())
        })
    }

    fn tor(&self, data_dir: &Path) -> Result<&tor::Tor, String> {
        self.tor.get_or_init(|| tor::Tor::start(data_dir)).as_ref().map_err(Clone::clone)
    }

    /// Act as if the gateway were listening on `addr`, without a socket.
    #[cfg(test)]
    pub(crate) fn pretend_gateway(&self, addr: SocketAddr) {
        let _ = self.gateway.set(addr);
        let _ = self.address.set(addr.to_string());
    }

    /// A new pairing code, or `None` while remote access is off.
    pub(crate) fn issue_code(&self, by: Option<String>) -> Option<String> {
        let address = self.address.get()?;
        let secret = self.offers().issue(by, Instant::now());
        Some(offers::code(&secret, address))
    }
}

impl Node {
    /// Turn on remote access for the next [`Node::serve`]. Call once.
    pub fn set_remote(&self, config: RemoteConfig) -> Result<(), Error> {
        self.remote.config.set(config).map_err(|_| Error::Detail("remote access is already configured".into()))
    }
}

/// Run the gateway (and the onion service) until `stop`. Returns at once
/// when remote access is off.
pub(crate) fn run(node: &Node, main_port: u16, stop: &AtomicBool) {
    let remote = &node.remote;
    let config = remote.config();
    if !config.serves() {
        return;
    }
    let bind = config.listen.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
    let listener = match TcpListener::bind(bind) {
        Ok(listener) => listener,
        Err(err) => return eprintln!("ob-node: remote gateway could not listen on {bind}: {err}"),
    };
    let Ok(local) = listener.local_addr() else {
        return;
    };
    let _ = remote.gateway.set(local);
    if config.onion {
        let onion = remote.tor(&node.data_dir).and_then(|tor| tor.serve(ONION_NICKNAME, local));
        match onion {
            Ok(forward) => {
                let address = forward.onion_address().map(|onion| onion.to_string());
                let _ = remote.onion.set(forward);
                match address {
                    Some(address) => {
                        eprintln!("ob-node: remote access at http://{address}/ (Tor only; state {})", remote.onion.get().map(|o| o.state()).unwrap_or_default());
                        let _ = remote.address.set(address);
                    }
                    None => eprintln!("ob-node: remote access: the onion service has no address yet"),
                }
            }
            Err(err) => eprintln!("ob-node: remote access is off: Tor: {err}"),
        }
    } else {
        eprintln!("ob-node: remote gateway on http://{local}/ (loopback only, no onion)");
        let _ = remote.address.set(local.to_string());
    }
    if remote.address.get().is_some() && remote.devices().list().is_empty() {
        if let Some(code) = remote.issue_code(None) {
            pair_cli::announce(&code, "No device is paired yet. Pair your first device with this code");
        }
    }
    gateway::serve(node, listener, main_port, stop);
}

/// Requests from the gateway and the loopback server enter the router one
/// at a time.
pub(crate) fn dispatch_lock() -> MutexGuard<'static, ()> {
    static DISPATCH: Mutex<()> = Mutex::new(());
    DISPATCH.lock().unwrap_or_else(|err| err.into_inner())
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
