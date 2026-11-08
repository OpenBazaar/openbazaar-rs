//! The node's onion services.
//!
//! * The admin service, restricted to the discovery keys of paired devices.
//!   Its address has a generation: 0 is nickname `ob-home`, later ones
//!   `ob-home-{n}`. An address move runs the old and the new generation side
//!   by side until every remaining device has reached the new one, or for
//!   [`MOVE_GRACE_SECS`], then retires the old one and deletes its keys.
//! * One open pairing service per live code, keyed by the code's secret, so
//!   a device that knows only the 8 words can find it. It serves `/pair/*`
//!   and goes away with the code.
//! * One public sale service, nickname `ob-sale`, with no client auth. It
//!   forwards to the sale gate, which allows catalog, media, reviews, order
//!   mail, and escrow mail, and answers everything else itself.
//! * One public Lightning service, nickname `ob-ln`. Virtual port 9735
//!   forwards to the loopback socket LDK binds. The node announces that
//!   onion and not the loopback address.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use tormail::Discovery;

use super::devices::Move;
use super::now_secs;
use super::pairing::Code;
use super::protocol::pairing_onion_seed;
use super::tor::{Service, Tor, ADMIN_PORT, LIGHTNING_PORT};
use crate::Node;

pub(crate) const MOVE_GRACE_SECS: u64 = 7 * 24 * 3600;

#[derive(Default)]
pub(crate) struct Services {
    admin: Mutex<Vec<(u32, Service)>>,
    pairing: Mutex<HashMap<String, Service>>,
    sale: Mutex<Option<Service>>,
    /// Loopback address the sale gate is listening on. Reply onions forward here.
    sale_target: Mutex<Option<SocketAddr>>,
    lightning: Mutex<Option<Service>>,
    replies: Mutex<HashMap<String, Service>>,
}

fn admin_nickname(generation: u32) -> String {
    if generation == 0 {
        "ob-home".to_string()
    } else {
        format!("ob-home-{generation}")
    }
}

fn pairing_nickname(code_id: &str) -> String {
    format!("ob-pair-{code_id}")
}

/// Slug for one reply onion. Ten seed bytes keep it unique and a valid slug.
fn reply_nickname(seed: &[u8; 32]) -> String {
    let hex: String = seed.iter().take(10).map(|byte| format!("{byte:02x}")).collect();
    format!("r{hex}")
}

impl Services {
    fn admin(&self) -> MutexGuard<'_, Vec<(u32, Service)>> {
        self.admin.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn pairing(&self) -> MutexGuard<'_, HashMap<String, Service>> {
        self.pairing.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn sale(&self) -> MutexGuard<'_, Option<Service>> {
        self.sale.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn lightning(&self) -> MutexGuard<'_, Option<Service>> {
        self.lightning.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn sale_target(&self) -> MutexGuard<'_, Option<SocketAddr>> {
        self.sale_target.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn replies(&self) -> MutexGuard<'_, HashMap<String, Service>> {
        self.replies.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub(crate) fn open_admin(
        &self,
        tor: &Tor,
        generation: u32,
        previous: Option<u32>,
        target: SocketAddr,
    ) {
        for generation in previous.into_iter().chain([generation]) {
            match tor.serve(
                &admin_nickname(generation),
                None,
                Discovery::Restricted(&[]),
                target,
            ) {
                Ok(service) => self.admin().push((generation, service)),
                Err(err) => eprintln!("ob-node: admin onion {generation}: {err}"),
            }
        }
    }

    /// The current admin address, `….onion`.
    pub(crate) fn onion(&self) -> Option<String> {
        self.admin()
            .iter()
            .max_by_key(|(generation, _)| *generation)
            .and_then(|(_, service)| service.forward.onion_address())
            .map(|onion| onion.to_string())
    }

    /// The generation of the admin service whose address is `host`.
    pub(crate) fn generation_of(&self, host: &str) -> Option<u32> {
        let host = host.strip_suffix(":80").unwrap_or(host);
        self.admin()
            .iter()
            .find(|(_, service)| {
                service
                    .forward
                    .onion_address()
                    .is_some_and(|onion| onion.to_string() == host)
            })
            .map(|(generation, _)| *generation)
    }

    pub(crate) fn state(&self) -> Option<String> {
        self.admin()
            .last()
            .map(|(_, service)| service.forward.state())
    }

    pub(crate) fn set_clients(&self, clients: &[(String, String)]) {
        for (generation, service) in self.admin().iter() {
            if let Err(err) = service.forward.set_clients(clients) {
                eprintln!("ob-node: admin onion {generation}: could not update its devices: {err}");
            }
        }
    }

    /// A public onion for catalog, order mail, and reviews. Its keys stay
    /// under the nickname `ob-sale`, so the address survives a restart.
    pub(crate) fn open_sale(&self, tor: &Tor, target: SocketAddr) {
        match tor.serve("ob-sale", None, Discovery::Open, target) {
            Ok(service) => {
                *self.sale_target() = Some(target);
                *self.sale() = Some(service);
            }
            Err(err) => eprintln!("ob-node: store onion: {err}"),
        }
    }

    /// Publish one reply onion on the existing Tor client. Virtual port 80
    /// forwards to the sale gate, the same allowlist as the store onion.
    /// The address is `onion_for_seed(seed)`. A second call with the same
    /// seed keeps the service already running.
    pub(crate) fn open_reply(&self, tor: &Tor, seed: &[u8; 32]) -> Result<String, String> {
        let target = (*self.sale_target()).ok_or_else(|| "the sale gate is not listening".to_string())?;
        let expected = tormail::onion_for_seed(seed).to_string();
        let nickname = reply_nickname(seed);
        if let Some(service) = self.replies().get(&nickname) {
            let got = service.forward.onion_address().map(|onion| onion.to_string()).unwrap_or_default();
            if got != expected {
                return Err(format!("reply onion {nickname} is {got}, not {expected}"));
            }
            return Ok(format!("http://{expected}"));
        }
        let service = tor.serve_on(&nickname, Some(*seed), Discovery::Open, ADMIN_PORT, target)?;
        let got = service
            .forward
            .onion_address()
            .map(|onion| onion.to_string())
            .ok_or_else(|| "reply onion has no address yet".to_string())?;
        if got != expected {
            return Err(format!("reply onion came up as {got}, not {expected}"));
        }
        self.replies().insert(nickname, service);
        Ok(format!("http://{expected}"))
    }

    /// A public onion for inbound Lightning. Its keys stay under `ob-ln`.
    /// Virtual port [`LIGHTNING_PORT`] forwards to `target` (loopback or LAN).
    pub(crate) fn open_lightning(&self, tor: &Tor, target: SocketAddr) -> Result<String, String> {
        let mut slot = self.lightning();
        if let Some(service) = slot.as_ref() {
            return service
                .forward
                .onion_address()
                .map(|onion| onion.to_string())
                .ok_or_else(|| "Lightning onion has no address yet".to_string());
        }
        let service = tor.serve_on("ob-ln", None, Discovery::Open, LIGHTNING_PORT, target)?;
        let onion = service
            .forward
            .onion_address()
            .map(|onion| onion.to_string())
            .ok_or_else(|| "Lightning onion has no address yet".to_string())?;
        *slot = Some(service);
        Ok(onion)
    }

    pub(crate) fn has_sale(&self) -> bool {
        self.sale().is_some()
    }

    /// The public store address, `….onion`, when the sale service is up.
    pub(crate) fn sale_onion(&self) -> Option<String> {
        self.sale()
            .as_ref()
            .and_then(|service| service.forward.onion_address())
            .map(|onion| onion.to_string())
    }

    pub(crate) fn open_pairing(&self, tor: &Tor, code: &Code, target: SocketAddr) {
        match tor.serve(
            &pairing_nickname(&code.id),
            Some(pairing_onion_seed(&code.secret)),
            Discovery::Open,
            target,
        ) {
            Ok(service) => {
                self.pairing().insert(code.id.clone(), service);
            }
            Err(err) => eprintln!("ob-node: pairing onion: {err}"),
        }
    }

    pub(crate) fn close_pairing_except(&self, live: &[Code], data_dir: &Path) {
        let gone: Vec<String> = self
            .pairing()
            .keys()
            .filter(|id| !live.iter().any(|code| &code.id == *id))
            .cloned()
            .collect();
        for id in gone {
            self.pairing().remove(&id);
            forget_keys(data_dir, &pairing_nickname(&id));
        }
    }

    fn retire(&self, generation: u32, data_dir: &Path) {
        self.admin().retain(|(live, _)| *live != generation);
        forget_keys(data_dir, &admin_nickname(generation));
    }
}

/// Delete a retired service's keys and state from Arti's directories.
fn forget_keys(data_dir: &Path, nickname: &str) {
    for dir in ["tor/state/keystore/hss", "tor/state/hss"] {
        let _ = std::fs::remove_dir_all(data_dir.join(dir).join(nickname));
    }
}

/// Start moving the admin service to a fresh address. A move already under
/// way ends first: its old address is retired now.
pub(crate) fn rotate(node: &Node) -> Result<String, String> {
    let remote = &node.remote;
    let (Some(tor), Some(target)) = (remote.running_tor(), remote.gateway.get().copied()) else {
        return Err("The node is not running an onion service.".into());
    };
    let next = {
        let mut devices = remote.devices();
        if let Some(moving) = devices.moving.take() {
            remote.services.retire(moving.from, &node.data_dir);
        }
        devices.moving = Some(Move {
            from: devices.generation,
            started: now_secs(),
        });
        devices.generation += 1;
        devices.save().map_err(|err| err.to_string())?;
        devices.generation
    };
    let service = tor.serve(
        &admin_nickname(next),
        None,
        Discovery::Restricted(&[]),
        target,
    )?;
    remote.services.admin().push((next, service));
    remote.refresh_discovery();
    remote
        .services
        .onion()
        .ok_or_else(|| "The new onion service has no address yet.".into())
}

pub(crate) fn finish_move(node: &Node) {
    let remote = &node.remote;
    let done = {
        let devices = remote.devices();
        match devices.moving {
            Some(moving) => {
                let everyone = devices
                    .list()
                    .iter()
                    .all(|device| device.generation >= devices.generation);
                (everyone || now_secs() > moving.started + MOVE_GRACE_SECS).then_some(moving.from)
            }
            None => None,
        }
    };
    if let Some(from) = done {
        remote.services.retire(from, &node.data_dir);
        let mut devices = remote.devices();
        devices.moving = None;
        if let Err(err) = devices.save() {
            eprintln!("ob-node: remote devices: {err}");
        }
        eprintln!(
            "ob-node: retired the old onion address; remote access is at {}",
            remote.services.onion().unwrap_or_default()
        );
    }
}
