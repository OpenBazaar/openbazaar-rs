//! The buyer's copy of an order.
//!
//! The sealed letter goes to the vendor. This record stays on the buyer's
//! node so the shop can show the order again. It is sealed at rest with the
//! same vault as the address book. It keeps the address label, not the street.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::vault::Vault;
use crate::Error;

const FILE: &str = "orders.obv";

#[derive(Clone)]
pub(crate) struct BuyerOrder {
    pub id: String,
    pub checkout_id: String,
    pub created: u64,
    pub seller: String,
    pub store_id: String,
    pub lines: Vec<Value>,
    pub items_sats: u64,
    pub shipping_sats: u64,
    pub total_sats: u64,
    pub settlement: String,
    pub payment: Option<Value>,
    pub escrow: Option<Value>,
    pub address_label: String,
    pub status: String,
}

impl BuyerOrder {
    fn to_json(&self) -> Value {
        let mut value = json!({
            "id": self.id,
            "checkoutId": self.checkout_id,
            "created": self.created,
            "seller": self.seller,
            "storeId": self.store_id,
            "lines": self.lines,
            "itemsSats": self.items_sats,
            "shippingSats": self.shipping_sats,
            "totalSats": self.total_sats,
            "settlement": self.settlement,
            "addressLabel": self.address_label,
            "status": self.status,
        });
        if let Some(payment) = &self.payment {
            value["payment"] = payment.clone();
        }
        if let Some(escrow) = &self.escrow {
            value["escrow"] = escrow.clone();
        }
        value
    }
}

pub(crate) struct BuyerOrders {
    path: std::path::PathBuf,
    vault: Vault,
    items: Vec<BuyerOrder>,
}

impl BuyerOrders {
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
        let mut items = self.items.clone();
        items.sort_by(|left, right| right.created.cmp(&left.created).then(right.id.cmp(&left.id)));
        json!({ "orders": items.iter().map(BuyerOrder::to_json).collect::<Vec<_>>() })
    }

    pub(crate) fn get_value(&self, id: &str) -> Option<Value> {
        self.items.iter().find(|item| item.id == id).map(BuyerOrder::to_json)
    }

    pub(crate) fn record(&mut self, order: BuyerOrder) -> Result<(), String> {
        self.items.retain(|item| item.id != order.id);
        self.items.push(order);
        self.flush()
    }

    fn flush(&self) -> Result<(), String> {
        let plain = serde_json::to_vec(&self.items.iter().map(BuyerOrder::to_json).collect::<Vec<_>>())
            .map_err(|err| err.to_string())?;
        let sealed = self.vault.seal(&plain).map_err(|err| err.to_string())?;
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, &sealed).map_err(|err| err.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|err| err.to_string())?;
        Ok(())
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn decode(bytes: &[u8]) -> Result<Vec<BuyerOrder>, Error> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| Error::Detail("the order list could not be read".to_string()))?;
    let Some(list) = value.as_array() else {
        return Err(Error::Detail("the order list could not be read".to_string()));
    };
    list.iter()
        .map(|item| {
            let id = item.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let lines = item.get("lines").and_then(Value::as_array).cloned().unwrap_or_default();
            Ok(BuyerOrder {
                id,
                checkout_id: item
                    .get("checkoutId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                created: item.get("created").and_then(Value::as_u64).unwrap_or(0),
                seller: item.get("seller").and_then(Value::as_str).unwrap_or("").to_string(),
                store_id: item.get("storeId").and_then(Value::as_str).unwrap_or("").to_string(),
                lines,
                items_sats: item.get("itemsSats").and_then(Value::as_u64).unwrap_or(0),
                shipping_sats: item.get("shippingSats").and_then(Value::as_u64).unwrap_or(0),
                total_sats: item.get("totalSats").and_then(Value::as_u64).unwrap_or(0),
                settlement: item.get("settlement").and_then(Value::as_str).unwrap_or("").to_string(),
                payment: item.get("payment").cloned().filter(|value| !value.is_null()),
                escrow: item.get("escrow").cloned().filter(|value| !value.is_null()),
                address_label: item
                    .get("addressLabel")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                status: item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("Awaiting payment")
                    .to_string(),
            })
        })
        .collect()
}
