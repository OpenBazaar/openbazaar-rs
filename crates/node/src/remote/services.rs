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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use tormail::Discovery;

use super::devices::Move;
use super::now_secs;
use super::pairing::Code;
use super::protocol::pairing_onion_seed;
use super::tor::{Service, Tor};
use crate::Node;

pub(crate) const MOVE_GRACE_SECS: u64 = 7 * 24 * 3600;

#[derive(Default)]
pub(crate) struct Services {
    admin: Mutex<Vec<(u32, Service)>>,
    pairing: Mutex<HashMap<String, Service>>,
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

impl Services {
    fn admin(&self) -> MutexGuard<'_, Vec<(u32, Service)>> {
        self.admin.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn pairing(&self) -> MutexGuard<'_, HashMap<String, Service>> {
        self.pairing.lock().unwrap_or_else(|err| err.into_inner())
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
