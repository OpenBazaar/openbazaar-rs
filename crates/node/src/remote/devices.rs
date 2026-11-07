//! Paired devices and the node key, sealed in the vault as `remote-devices.obv`.
//!
//! A device is a signing key (and optionally a watch key for background
//! sync and a Tor discovery key), a name the user chose, and the device that
//! paired it (none for the node's own console or app). Removing a device also
//! removes every device it paired, so a stolen phone cannot leave behind a
//! device of the thief's. The node key is the X25519 key of the LAN channel
//! and the pairing transcript; the node id that every signature names is
//! derived from it.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::Zeroizing;

use super::auth::decode_hex;
use super::protocol::{node_id, DeviceKey};
use crate::vault::Vault;
use crate::vault_key::{encode_hex, random_bytes};
use crate::Error;

const FILE: &str = "remote-devices.obv";
pub(crate) const MAX_DEVICES: usize = 32;
pub(crate) const MAX_NAME_CHARS: usize = 40;
/// `lastSeen` is written to disk at most this often per device.
const SEEN_WRITE_SECS: u64 = 600;
pub(crate) const PLATFORMS: [&str; 4] = ["ios", "android", "desktop", "browser"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Push {
    pub platform: String,
    pub token: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub id: String,
    pub key: DeviceKey,
    pub watch: Option<DeviceKey>,
    pub discovery: Option<String>,
    pub name: String,
    pub platform: String,
    pub paired_at: u64,
    pub last_seen: u64,
    pub last_via: Option<String>,
    pub paired_by: Option<String>,
    pub push: Option<Push>,
    /// The newest onion generation this device has reached the node on.
    pub generation: u32,
    seen_on_disk: u64,
}

/// What a new device brings to pairing.
pub(crate) struct NewDevice {
    pub key: DeviceKey,
    pub watch: Option<DeviceKey>,
    pub discovery: Option<String>,
    pub name: String,
    pub platform: String,
    pub paired_by: Option<String>,
}

impl Device {
    pub(crate) fn kind(&self) -> &'static str {
        if self.platform == "browser" {
            "browser"
        } else {
            "app"
        }
    }

    pub(crate) fn value(&self, this: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "platform": self.platform,
            "kind": self.kind(),
            "pairedAt": self.paired_at,
            "lastSeen": self.last_seen,
            "lastVia": self.last_via,
            "pairedBy": self.paired_by,
            "push": { "enabled": self.push.is_some(), "platform": self.push.as_ref().map(|push| push.platform.clone()) },
            "thisDevice": this == Some(self.id.as_str()),
        })
    }

    fn stored(&self) -> Value {
        json!({
            "key": self.key.text(),
            "watchKey": self.watch.as_ref().map(DeviceKey::text),
            "discoveryKey": self.discovery,
            "name": self.name,
            "platform": self.platform,
            "pairedAt": self.paired_at,
            "lastSeen": self.last_seen,
            "lastVia": self.last_via,
            "pairedBy": self.paired_by,
            "push": self.push.as_ref().map(|push| json!({ "platform": push.platform, "token": push.token })),
            "generation": self.generation,
        })
    }

    fn load(item: &Value) -> Option<Self> {
        let key = DeviceKey::parse(item["key"].as_str()?)?;
        let last_seen = item["lastSeen"].as_u64().unwrap_or(0);
        Some(Device {
            id: key.id(),
            watch: item["watchKey"].as_str().and_then(DeviceKey::parse),
            key,
            discovery: item["discoveryKey"].as_str().map(String::from),
            name: item["name"].as_str()?.to_string(),
            platform: item["platform"].as_str().unwrap_or("desktop").to_string(),
            paired_at: item["pairedAt"].as_u64().unwrap_or(0),
            last_seen,
            last_via: item["lastVia"].as_str().map(String::from),
            paired_by: item["pairedBy"].as_str().map(String::from),
            push: item["push"].as_object().and_then(|push| {
                Some(Push {
                    platform: push.get("platform")?.as_str()?.to_string(),
                    token: push.get("token")?.as_str()?.to_string(),
                })
            }),
            generation: item["generation"].as_u64().unwrap_or(0) as u32,
            seen_on_disk: last_seen,
        })
    }
}

/// An onion address move in progress: devices are told `nextOnion` until
/// each one has reached the new address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Move {
    pub from: u32,
    pub started: u64,
}

pub(crate) struct Devices {
    path: PathBuf,
    vault: Vault,
    node_secret: Zeroizing<[u8; 32]>,
    pub node_public: [u8; 32],
    pub node_id: String,
    /// The current admin onion generation (0 is nickname `ob-home`).
    pub generation: u32,
    pub moving: Option<Move>,
    list: Vec<Device>,
}

impl Devices {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let unreadable = || Error::Detail("remote-devices.obv could not be read".into());
        let Some(bytes) = vault.read_sealed(&path)? else {
            let mut devices = Self::empty(path, vault, Zeroizing::new(random_bytes::<32>()));
            devices.save()?;
            return Ok(devices);
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unreadable())?;
        let secret: [u8; 32] = value["nodeKey"]
            .as_str()
            .and_then(decode_hex)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(unreadable)?;
        let mut devices = Self::empty(path, vault, Zeroizing::new(secret));
        devices.generation = value["generation"].as_u64().unwrap_or(0) as u32;
        devices.moving = value["moving"].as_object().and_then(|moving| {
            Some(Move {
                from: moving.get("from")?.as_u64()? as u32,
                started: moving.get("started")?.as_u64()?,
            })
        });
        devices.list = value["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Device::load)
            .collect();
        Ok(devices)
    }

    fn empty(path: PathBuf, vault: Vault, node_secret: Zeroizing<[u8; 32]>) -> Self {
        let node_public = *XPublic::from(&StaticSecret::from(*node_secret)).as_bytes();
        Self {
            path,
            vault,
            node_id: node_id(&node_public),
            node_secret,
            node_public,
            generation: 0,
            moving: None,
            list: Vec::new(),
        }
    }

    pub(crate) fn node_secret(&self) -> StaticSecret {
        StaticSecret::from(*self.node_secret)
    }

    pub(crate) fn save(&mut self) -> Result<(), Error> {
        let body = json!({
            "v": 2,
            "nodeKey": encode_hex(&*self.node_secret),
            "generation": self.generation,
            "moving": self.moving.map(|moving| json!({ "from": moving.from, "started": moving.started })),
            "devices": self.list.iter().map(Device::stored).collect::<Vec<_>>(),
        });
        self.vault
            .write_sealed(&self.path, Zeroizing::new(body.to_string()).as_bytes())?;
        for device in &mut self.list {
            device.seen_on_disk = device.last_seen;
        }
        Ok(())
    }

    pub(crate) fn list(&self) -> &[Device] {
        &self.list
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Device> {
        self.list.iter().find(|device| device.id == id)
    }

    /// The device whose key or watch key has `id`, and whether it was the watch key.
    pub(crate) fn signer(&self, id: &str) -> Option<(&Device, bool)> {
        self.list.iter().find_map(|device| {
            if device.id == id {
                Some((device, false))
            } else {
                device
                    .watch
                    .as_ref()
                    .filter(|watch| watch.id() == id)
                    .map(|_| (device, true))
            }
        })
    }

    pub(crate) fn add(&mut self, new: NewDevice, now: u64) -> Result<Device, String> {
        let id = new.key.id();
        if self.signer(&id).is_some()
            || new
                .watch
                .as_ref()
                .is_some_and(|watch| self.signer(&watch.id()).is_some())
        {
            return Err("That device is already paired.".into());
        }
        if self.list.len() >= MAX_DEVICES {
            return Err(format!(
                "This node already has {MAX_DEVICES} devices. Remove one first."
            ));
        }
        let device = Device {
            id,
            key: new.key,
            watch: new.watch,
            discovery: new.discovery,
            name: new.name,
            platform: new.platform,
            paired_at: now,
            last_seen: now,
            last_via: None,
            paired_by: new.paired_by,
            push: None,
            generation: self.generation,
            seen_on_disk: now,
        };
        self.list.push(device.clone());
        self.save().map_err(|err| err.to_string())?;
        Ok(device)
    }

    /// Remove `id` and every device it paired, directly or through another.
    /// Returns the ids removed; empty when `id` is not paired.
    pub(crate) fn revoke(&mut self, id: &str) -> Result<Vec<String>, Error> {
        if self.get(id).is_none() {
            return Ok(Vec::new());
        }
        let mut gone = vec![id.to_string()];
        let mut index = 0;
        while index < gone.len() {
            let parent = gone[index].clone();
            for device in &self.list {
                if device.paired_by.as_deref() == Some(parent.as_str())
                    && !gone.contains(&device.id)
                {
                    gone.push(device.id.clone());
                }
            }
            index += 1;
        }
        self.list.retain(|device| !gone.contains(&device.id));
        self.save()?;
        Ok(gone)
    }

    pub(crate) fn touch(&mut self, id: &str, via: &str, generation: Option<u32>, now: u64) {
        let Some(device) = self.list.iter_mut().find(|device| device.id == id) else {
            return;
        };
        let moved = generation.is_some_and(|generation| generation > device.generation);
        let changed = moved || device.last_via.as_deref() != Some(via);
        device.last_seen = now;
        device.last_via = Some(via.to_string());
        if let Some(generation) = generation.filter(|_| moved) {
            device.generation = generation;
        }
        if changed || now >= device.seen_on_disk + SEEN_WRITE_SECS {
            if let Err(err) = self.save() {
                eprintln!("ob-node: remote devices: {err}");
            }
        }
    }

    pub(crate) fn set_push(&mut self, id: &str, push: Option<Push>) -> Result<(), Error> {
        if let Some(device) = self.list.iter_mut().find(|device| device.id == id) {
            device.push = push;
        }
        self.save()
    }

    /// The restricted-discovery list for the admin onion: `(nickname, key)`.
    pub(crate) fn discovery_clients(&self) -> Vec<(String, String)> {
        self.list
            .iter()
            .filter_map(|device| Some((format!("d{}", device.id), device.discovery.clone()?)))
            .collect()
    }
}

/// Trimmed, 1 to [`MAX_NAME_CHARS`] characters, no control characters.
pub(crate) fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let count = name.chars().count();
    (count > 0 && count <= MAX_NAME_CHARS && !name.chars().any(char::is_control))
        .then(|| name.to_string())
}
