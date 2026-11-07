//! Paired devices, sealed in the vault as `remote-devices.obv`.
//!
//! A device is a public key with a name the user chose and the device that
//! paired it (none for the node's own console or desktop app). Revoking a
//! device also revokes every device it paired, so a stolen phone cannot
//! leave behind a device of the thief's. The file also holds the node id
//! that every request signature names.

use std::path::{Path, PathBuf};

use bitcoin::secp256k1::XOnlyPublicKey;
use serde_json::{json, Value};

use super::auth::{device_id, parse_key};
use crate::vault::Vault;
use crate::vault_key::{encode_hex, random_bytes};
use crate::Error;

const FILE: &str = "remote-devices.obv";
pub(crate) const MAX_DEVICES: usize = 32;
pub(crate) const MAX_NAME_CHARS: usize = 40;
/// `lastSeen` is written to disk at most this often per device.
const SEEN_WRITE_SECS: u64 = 600;

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub id: String,
    pub key: XOnlyPublicKey,
    pub name: String,
    pub paired_at: u64,
    pub last_seen: u64,
    pub paired_by: Option<String>,
    seen_on_disk: u64,
}

impl Device {
    pub(crate) fn value(&self, this: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "pairedAt": self.paired_at,
            "lastSeen": self.last_seen,
            "pairedBy": self.paired_by,
            "thisDevice": this == Some(self.id.as_str()),
        })
    }
}

pub(crate) struct Devices {
    path: PathBuf,
    vault: Vault,
    pub node_id: String,
    list: Vec<Device>,
}

impl Devices {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let Some(bytes) = vault.read_sealed(&path)? else {
            return Ok(Self { path, vault, node_id: encode_hex(&random_bytes::<16>()), list: Vec::new() });
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail("remote-devices.obv could not be read".into()))?;
        let node_id = value["nodeId"].as_str().unwrap_or_default().to_string();
        if node_id.len() != 32 {
            return Err(Error::Detail("remote-devices.obv has no node id".into()));
        }
        let list = value["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let key = parse_key(item["key"].as_str()?)?;
                let last_seen = item["lastSeen"].as_u64().unwrap_or(0);
                Some(Device {
                    id: device_id(&key),
                    key,
                    name: item["name"].as_str()?.to_string(),
                    paired_at: item["pairedAt"].as_u64().unwrap_or(0),
                    last_seen,
                    paired_by: item["pairedBy"].as_str().map(String::from),
                    seen_on_disk: last_seen,
                })
            })
            .collect();
        Ok(Self { path, vault, node_id, list })
    }

    fn save(&mut self) -> Result<(), Error> {
        let devices: Vec<Value> = self
            .list
            .iter()
            .map(|device| {
                json!({
                    "key": encode_hex(&device.key.serialize()),
                    "name": device.name,
                    "pairedAt": device.paired_at,
                    "lastSeen": device.last_seen,
                    "pairedBy": device.paired_by,
                })
            })
            .collect();
        let body = json!({ "v": 1, "nodeId": self.node_id, "devices": devices });
        self.vault.write_sealed(&self.path, body.to_string().as_bytes())?;
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

    pub(crate) fn add(&mut self, key: XOnlyPublicKey, name: &str, paired_by: Option<String>, now: u64) -> Result<Device, String> {
        let id = device_id(&key);
        if self.get(&id).is_some() {
            return Err("That device is already paired.".into());
        }
        if self.list.len() >= MAX_DEVICES {
            return Err(format!("This node already has {MAX_DEVICES} devices. Remove one first."));
        }
        let device = Device { id, key, name: name.to_string(), paired_at: now, last_seen: now, paired_by, seen_on_disk: now };
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
                if device.paired_by.as_deref() == Some(parent.as_str()) && !gone.contains(&device.id) {
                    gone.push(device.id.clone());
                }
            }
            index += 1;
        }
        self.list.retain(|device| !gone.contains(&device.id));
        self.save()?;
        Ok(gone)
    }

    pub(crate) fn touch(&mut self, id: &str, now: u64) {
        let Some(device) = self.list.iter_mut().find(|device| device.id == id) else {
            return;
        };
        device.last_seen = now;
        if now >= device.seen_on_disk + SEEN_WRITE_SECS {
            if let Err(err) = self.save() {
                eprintln!("ob-node: remote devices: {err}");
            }
        }
    }
}

/// Trimmed, 1 to [`MAX_NAME_CHARS`] characters, no control characters.
pub(crate) fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let count = name.chars().count();
    (count > 0 && count <= MAX_NAME_CHARS && !name.chars().any(char::is_control)).then(|| name.to_string())
}
