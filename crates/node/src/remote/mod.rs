//! Remote access to a home node from paired devices. Design and threat model:
//! `docs/home-node.md`; wire format: `docs/home-node-protocol.md`.
//!
//! As a server (`ob-node serve --remote`), the node publishes its admin API
//! as a restricted onion service that only paired devices can find
//! ([`services`]), and pipes it to a loopback [`gateway`]. A device pairs
//! with a one-time code, its request is approved on the node after the owner
//! compares emoji ([`pairing`]), and it is registered with its own keys
//! ([`devices`]); from then on it signs every request ([`auth`]). With
//! `--lan` the same signed requests also arrive over an encrypted channel on
//! the local network ([`lan`]). There is no account and no relay.
//!
//! As a client, a node (the desktop app's sidecar) holds one device key for
//! one home node and forwards `/home/proxy/*` to it ([`home`]), so the same
//! shop window can manage a node elsewhere.

mod auth;
mod batch;
mod devices;
mod gateway;
mod home;
mod home_link;
mod home_pair;
mod lan;
mod local_http;
mod pair_cli;
mod pair_http;
mod pairing;
mod policy;
mod protocol;
mod push;
mod services;
mod sync;
mod tor;
mod web;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod vectors;

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::vault::Vault;
use crate::{Error, Node};

pub(crate) use home::{is_proxy_request, response as home_response};
pub(crate) use local_http::response as local_response;
pub use pair_cli::{devices_command, pair_command};

/// Set by the gateway on requests it forwards; names the signing device.
pub(crate) const REMOTE_DEVICE_HEADER: &str = "X-Ob-Remote-Device";
/// Set by the gateway: `tor:{generation}`, `lan`, or `loopback`.
pub(crate) const REMOTE_VIA_HEADER: &str = "X-Ob-Remote-Via";
pub(crate) const LAN_PORT: u16 = 8790;
/// On answers to signed requests while the node is moving: its new address.
pub(crate) const NEXT_ONION_HEADER: &str = "X-Ob-Next-Onion";

/// `serve` flags for remote access, taken out of the argument list before
/// the other flags are parsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteConfig {
    /// `--remote`: publish the admin onion service.
    pub onion: bool,
    /// `--remote-listen 127.0.0.1:PORT`: the gateway's own address. Without
    /// `--remote` this is a loopback-only gateway for tests.
    pub listen: Option<SocketAddr>,
    /// `--lan` (all interfaces, port 8790) or `--lan-listen ADDR`: the
    /// encrypted LAN channel, the first-run setup page, and mDNS.
    pub lan: Option<SocketAddr>,
    /// `--web-dir DIR`: the web shop export served to remote browsers.
    pub web_dir: Option<PathBuf>,
    /// `--push-gateway http://….onion`: where content-free wake-ups go.
    pub push_gateway: Option<String>,
    /// `--allow-loopback-home`: as a client, accept a home node reached on a
    /// loopback LAN address. Tests only.
    pub loopback_homes: bool,
}

impl RemoteConfig {
    /// Split remote flags from `args`; the rest go to `ServeOptions::parse`.
    pub fn extract(args: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut config = Self::default();
        let mut rest = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value =
                |name: &str| iter.next().cloned().ok_or(format!("{name} needs a value"));
            match arg.as_str() {
                "--remote" => config.onion = true,
                "--allow-loopback-home" => config.loopback_homes = true,
                "--lan" => config.lan = Some(SocketAddr::from(([0, 0, 0, 0], LAN_PORT))),
                "--lan-listen" => {
                    let text = value("--lan-listen")?;
                    config.lan = Some(text.parse().map_err(|_| {
                        format!("--lan-listen must look like 0.0.0.0:{LAN_PORT}, not {text:?}")
                    })?);
                }
                "--remote-listen" => {
                    let text = value("--remote-listen")?;
                    let addr: SocketAddr = text.parse().map_err(|_| {
                        format!("--remote-listen must look like 127.0.0.1:8791, not {text:?}")
                    })?;
                    if !addr.ip().is_loopback() {
                        return Err("--remote-listen must be a loopback address. The gateway speaks plain HTTP; devices reach it over Tor or the encrypted LAN channel (--lan).".into());
                    }
                    config.listen = Some(addr);
                }
                "--web-dir" => config.web_dir = Some(PathBuf::from(value("--web-dir")?)),
                "--push-gateway" => {
                    let url = value("--push-gateway")?;
                    push::gateway_target(&url)?;
                    config.push_gateway = Some(url);
                }
                _ => rest.push(arg.clone()),
            }
        }
        Ok((config, rest))
    }

    fn serves(&self) -> bool {
        self.onion || self.listen.is_some() || self.lan.is_some()
    }
}

pub(crate) struct Remote {
    devices: Mutex<devices::Devices>,
    pairing: Mutex<pairing::Pairing>,
    pub(crate) replay: auth::ReplayGuard,
    pub(crate) batches: batch::Batches,
    pub(crate) push: push::PushState,
    config: OnceLock<RemoteConfig>,
    gateway: OnceLock<SocketAddr>,
    pairing_gateway: OnceLock<SocketAddr>,
    pub(crate) services: services::Services,
    tor: OnceLock<Result<tor::Tor, String>>,
    lan: lan::LanState,
    home: home::HomeState,
}

impl Remote {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        Ok(Self {
            devices: Mutex::new(devices::Devices::open(dir, vault.clone())?),
            pairing: Mutex::new(pairing::Pairing::default()),
            replay: auth::ReplayGuard::new(now_secs()),
            batches: batch::Batches::default(),
            push: push::PushState::default(),
            config: OnceLock::new(),
            gateway: OnceLock::new(),
            pairing_gateway: OnceLock::new(),
            services: services::Services::default(),
            tor: OnceLock::new(),
            lan: lan::LanState::default(),
            home: home::HomeState::open(dir, vault)?,
        })
    }

    fn config(&self) -> RemoteConfig {
        self.config.get().cloned().unwrap_or_default()
    }

    pub(crate) fn devices(&self) -> MutexGuard<'_, devices::Devices> {
        self.devices.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn pairing(&self) -> MutexGuard<'_, pairing::Pairing> {
        self.pairing.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn node_id(&self) -> String {
        self.devices().node_id.clone()
    }

    pub(crate) fn web_dir(&self) -> Option<&Path> {
        self.config
            .get()
            .and_then(|config| config.web_dir.as_deref())
    }

    pub(crate) fn enabled(&self) -> bool {
        self.gateway.get().is_some()
    }

    fn tor(&self, data_dir: &Path) -> Result<&tor::Tor, String> {
        self.tor
            .get_or_init(|| tor::Tor::start(data_dir))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn running_tor(&self) -> Option<&tor::Tor> {
        self.tor.get().and_then(|tor| tor.as_ref().ok())
    }

    /// Act as if the gateway were listening on `addr`, without a socket.
    #[cfg(test)]
    pub(crate) fn pretend_gateway(&self, addr: SocketAddr) {
        let _ = self.gateway.set(addr);
    }

    /// A new pairing code (and its pairing onion, when Tor is up), or `None`
    /// while remote access is off.
    pub(crate) fn issue_code(&self, by: Option<String>) -> Option<pairing::Code> {
        if !self.enabled() {
            return None;
        }
        let code = self.pairing().issue(by, Instant::now());
        if let (Some(tor), Some(target)) = (self.running_tor(), self.pairing_gateway.get()) {
            self.services.open_pairing(tor, &code, *target);
        }
        Some(code)
    }

    pub(crate) fn link_for(&self, code: &pairing::Code) -> String {
        let node_public = self.devices().node_public;
        pairing::link(
            &code.secret,
            &node_public,
            self.services.onion().as_deref(),
            &self.lan.addresses(),
        )
    }

    /// Tell the admin onion which discovery keys may find it.
    pub(crate) fn refresh_discovery(&self) {
        let clients = self.devices().discovery_clients();
        self.services.set_clients(&clients);
    }

    /// Remove `id` and the devices it paired, everywhere they are known.
    pub(crate) fn revoke(&self, id: &str) -> Result<Vec<String>, Error> {
        let removed = self.devices().revoke(id)?;
        if !removed.is_empty() {
            self.pairing().drop_from(&removed);
            for gone in &removed {
                self.replay.forget(gone);
                self.batches.forget(gone);
            }
            self.refresh_discovery();
        }
        Ok(removed)
    }
}

impl Node {
    /// Turn on remote access for the next [`Node::serve`]. Call once.
    pub fn set_remote(&self, config: RemoteConfig) -> Result<(), Error> {
        self.remote
            .config
            .set(config)
            .map_err(|_| Error::Detail("remote access is already configured".into()))
    }
}

/// Run the gateways, onion services, LAN channel, and upkeep until `stop`.
/// Returns at once when remote access is off.
pub(crate) fn run(node: &Node, main_port: u16, stop: &AtomicBool) {
    let remote = &node.remote;
    let config = remote.config();
    if !config.serves() {
        return;
    }
    let Some((admin, pairing)) = bind_gateways(config.listen) else {
        return;
    };
    let (Ok(admin_addr), Ok(pairing_addr)) = (admin.local_addr(), pairing.local_addr()) else {
        return;
    };
    let _ = remote.gateway.set(admin_addr);
    let _ = remote.pairing_gateway.set(pairing_addr);
    if config.onion {
        match remote.tor(&node.data_dir) {
            Ok(tor) => {
                let generation = remote.devices().generation;
                let moving = remote.devices().moving;
                remote.services.open_admin(
                    tor,
                    generation,
                    moving.map(|moving| moving.from),
                    admin_addr,
                );
                remote.refresh_discovery();
                match remote.services.onion() {
                    Some(onion) => eprintln!(
                        "ob-node: remote access at http://{onion}/ (Tor only, paired devices only)"
                    ),
                    None => {
                        eprintln!("ob-node: remote access: the onion service has no address yet")
                    }
                }
            }
            Err(err) => eprintln!("ob-node: remote access over Tor is off: {err}"),
        }
    } else {
        eprintln!("ob-node: remote gateway on http://{admin_addr}/ (loopback only, no onion)");
    }
    std::thread::scope(|scope| {
        if let Some(addr) = config.lan {
            scope.spawn(move || lan::serve(node, addr, main_port, stop));
        }
        scope
            .spawn(move || gateway::serve(node, pairing, gateway::Entry::Pairing, main_port, stop));
        scope.spawn(move || upkeep(node, main_port, stop));
        if remote.devices().list().is_empty() {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(code) = remote.issue_code(None) {
                pair_cli::announce(
                    &remote.link_for(&code),
                    &code,
                    "No device is paired yet. Pair your first device with this code",
                );
            }
        }
        gateway::serve(node, admin, gateway::Entry::Admin, main_port, stop);
    });
}

fn bind_gateways(listen: Option<SocketAddr>) -> Option<(TcpListener, TcpListener)> {
    let any = SocketAddr::from(([127, 0, 0, 1], 0));
    let bind = |addr: SocketAddr| {
        TcpListener::bind(addr)
            .map_err(|err| eprintln!("ob-node: remote gateway could not listen on {addr}: {err}"))
            .ok()
    };
    Some((bind(listen.unwrap_or(any))?, bind(any)?))
}

/// Retire pairing onions, finish address moves, and send push wake-ups.
fn upkeep(node: &Node, main_port: u16, stop: &AtomicBool) {
    let mut last = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        if last.elapsed() < Duration::from_secs(5) {
            continue;
        }
        last = Instant::now();
        let live = node.remote.pairing().live(Instant::now());
        node.remote
            .services
            .close_pairing_except(&live, &node.data_dir);
        services::finish_move(node);
        push::check(node, main_port);
    }
}

/// Requests from the gateway and the loopback server enter the router one
/// at a time.
pub(crate) fn dispatch_lock() -> MutexGuard<'static, ()> {
    static DISPATCH: Mutex<()> = Mutex::new(());
    DISPATCH.lock().unwrap_or_else(|err| err.into_inner())
}

/// The request target with its query string, which `http_parse` leaves out
/// of `path` but signatures cover.
pub(crate) fn request_target(head: &str) -> Option<&str> {
    head.lines().next()?.split(' ').nth(1)
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
