//! The buyer's address book. It lives only in this node's data directory.
//!
//! The file on disk is sealed by [`crate::vault`]. Nothing here is written
//! into the catalog, and a checkout copies one address into that one vendor's
//! letter.

use std::fs;
use std::path::Path;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde_json::{json, Map, Value};

use crate::vault::Vault;
use crate::Error;

const FILE: &str = "addresses.obv";
const FIELDS: &[&str] = &[
    "label",
    "name",
    "street",
    "street2",
    "city",
    "region",
    "postalCode",
    "country",
    "default",
];

#[derive(Clone)]
pub(crate) struct Address {
    pub id: String,
    pub label: String,
    pub name: String,
    pub street: String,
    pub street2: String,
    pub city: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
    pub default: bool,
}

impl Address {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "name": self.name,
            "street": self.street,
            "street2": self.street2,
            "city": self.city,
            "region": self.region,
            "postalCode": self.postal_code,
            "country": self.country,
            "default": self.default,
        })
    }

    pub(crate) fn letter_text(&self) -> String {
        let mut lines = vec![self.name.clone(), self.street.clone()];
        if !self.street2.is_empty() {
            lines.push(self.street2.clone());
        }
        lines.push(format!("{}, {} {}", self.city, self.region, self.postal_code));
        lines.push(self.country.clone());
        lines.join("\n")
    }
}

pub(crate) struct AddressBook {
    path: std::path::PathBuf,
    vault: Vault,
    items: Vec<Address>,
}

impl AddressBook {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let items = if path.exists() {
            let blob = fs::read(&path).map_err(|err| Error::Detail(err.to_string()))?;
            decode(&vault.open_bytes(&blob)?)?
        } else {
            Vec::new()
        };
        Ok(Self { path, vault, items })
    }

    pub(crate) fn list_value(&self) -> Value {
        json!({ "addresses": self.items.iter().map(Address::to_json).collect::<Vec<_>>() })
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Address> {
        self.items.iter().find(|item| item.id == id)
    }

    pub(crate) fn create(&mut self, body: &Value) -> Result<Value, String> {
        let mut address = parse_address(body, new_id())?;
        if self.items.is_empty() || address.default {
            for item in &mut self.items {
                item.default = false;
            }
            address.default = true;
        }
        let value = address.to_json();
        self.items.push(address);
        self.flush()?;
        Ok(value)
    }

    pub(crate) fn update(&mut self, id: &str, body: &Value) -> Result<Value, String> {
        let mut address = parse_address(body, id.to_string())?;
        if !self.items.iter().any(|item| item.id == id) {
            return Err("That address is not on this node.".to_string());
        }
        if address.default {
            for item in &mut self.items {
                item.default = false;
            }
        } else if self.items.iter().any(|item| item.id == id && item.default)
            && !self.items.iter().any(|item| item.id != id && item.default)
        {
            address.default = true;
        }
        let value = address.to_json();
        if let Some(slot) = self.items.iter_mut().find(|item| item.id == id) {
            *slot = address;
        }
        self.flush()?;
        Ok(value)
    }

    pub(crate) fn delete(&mut self, id: &str) -> Result<Value, String> {
        let before = self.items.len();
        let was_default = self.items.iter().any(|item| item.id == id && item.default);
        self.items.retain(|item| item.id != id);
        if self.items.len() == before {
            return Err("That address is not on this node.".to_string());
        }
        if was_default {
            if let Some(first) = self.items.first_mut() {
                first.default = true;
            }
        }
        self.flush()?;
        Ok(json!({ "deleted": id }))
    }

    fn flush(&self) -> Result<(), String> {
        let plain = serde_json::to_vec(&self.items.iter().map(Address::to_json).collect::<Vec<_>>())
            .map_err(|err| err.to_string())?;
        let sealed = self.vault.seal(&plain).map_err(|err| err.to_string())?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, &sealed).map_err(|err| err.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|err| err.to_string())?;
        Ok(())
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<Address>, Error> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::Detail("the address book could not be read".to_string()))?;
    let Some(list) = value.as_array() else {
        return Err(Error::Detail("the address book could not be read".to_string()));
    };
    list.iter()
        .map(|item| {
            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
            parse_address(item, id.to_string()).map_err(|err| Error::Detail(err))
        })
        .collect()
}

fn parse_address(body: &Value, id: String) -> Result<Address, String> {
    let Some(map) = body.as_object() else {
        return Err("The address could not be read.".to_string());
    };
    if map.keys().any(|key| !FIELDS.contains(&key.as_str()) && key != "id") {
        return Err("That field is not part of an address.".to_string());
    }
    if !id_ok(&id) {
        return Err("The address could not be read.".to_string());
    }
    Ok(Address {
        id,
        label: field(map, "label")?,
        name: field(map, "name")?,
        street: field(map, "street")?,
        street2: optional_field(map, "street2")?,
        city: field(map, "city")?,
        region: field(map, "region")?,
        postal_code: field(map, "postalCode")?,
        country: field(map, "country")?,
        default: map.get("default").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn field(map: &Map<String, Value>, key: &str) -> Result<String, String> {
    let text = optional_field(map, key)?;
    if text.is_empty() {
        Err("Enter the label, recipient, street, city, region, postal code, and country.".to_string())
    } else {
        Ok(text)
    }
}

fn optional_field(map: &Map<String, Value>, key: &str) -> Result<String, String> {
    match map.get(key) {
        None => Ok(String::new()),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.len() > 200 || text.as_bytes().contains(&0) {
                Err("That address is too long.".to_string())
            } else {
                Ok(text.to_string())
            }
        }
        Some(_) => Err("The address could not be read.".to_string()),
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    StdRng::from_os_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}
