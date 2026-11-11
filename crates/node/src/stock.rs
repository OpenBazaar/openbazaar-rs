//! Live stock for the own store's listings, and the reservations that keep
//! two buyers from paying for the last unit.
//!
//! The signed listing shows stock as of its last publish; replicas keep
//! serving it while this node is offline. This ledger is the authority: an
//! order letter reserves units for its payment window when it is read, a
//! payment turns the reservation into a sale, an unpaid quote gives its units
//! back when it expires. A payment that arrives after its units went to
//! someone else is oversold, and the sale is refunded (see
//! `docs/listing-parity.md`). Every change that moves what is available
//! returns the keys whose listings should be signed again.
//!
//! Kept in `stock.obv`, sealed with the vault key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openbazaar_protocol::{ListingId, VariantId};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::vault::Vault;
use crate::Error;

const FILE: &str = "stock.obv";
/// Settled reservations are kept this long, so a late payment still finds its own.
const KEEP_SETTLED: u64 = 30 * 24 * 60 * 60;

pub(crate) type Key = (ListingId, VariantId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Held,
    Sold,
    Released,
    Oversold,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Held => "held",
            State::Sold => "sold",
            State::Released => "released",
            State::Oversold => "oversold",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [State::Held, State::Sold, State::Released, State::Oversold].into_iter().find(|s| s.as_str() == text)
    }
}

#[derive(Clone, Debug)]
struct Reservation {
    order: String,
    key: Key,
    quantity: u32,
    expires_at: u64,
    state: State,
}

/// Why a reservation was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    Short { available: u32 },
    Failed(String),
}

/// What a payment did to its reservation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settled {
    Sold,
    /// Paid after the units were sold to someone else: refund it.
    Oversold,
}

pub(crate) struct StockLedger {
    path: PathBuf,
    vault: Vault,
    on_hand: BTreeMap<Key, u32>,
    reservations: Vec<Reservation>,
}

fn key_hex(key: &Key) -> (String, String) {
    (hex(&key.0 .0), hex(&key.1 .0))
}

fn key_of(item: &Value) -> Option<Key> {
    let listing = unhex(item["listing"].as_str()?).ok()?.try_into().ok()?;
    let variant = unhex(item["variant"].as_str()?).ok()?.try_into().ok()?;
    Some((ListingId(listing), VariantId(variant)))
}

impl StockLedger {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut ledger = Self { path, vault, on_hand: BTreeMap::new(), reservations: Vec::new() };
        let Some(bytes) = ledger.vault.read_sealed(&ledger.path)? else {
            return Ok(ledger);
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
        for item in value["onHand"].as_array().into_iter().flatten() {
            if let (Some(key), Some(count)) = (key_of(item), item["count"].as_u64()) {
                ledger.on_hand.insert(key, count as u32);
            }
        }
        for item in value["reservations"].as_array().into_iter().flatten() {
            let parsed = (|| {
                Some(Reservation {
                    order: item["order"].as_str()?.to_string(),
                    key: key_of(item)?,
                    quantity: item["quantity"].as_u64()? as u32,
                    expires_at: item["expiresAt"].as_u64()?,
                    state: State::parse(item["state"].as_str()?)?,
                })
            })();
            match parsed {
                Some(reservation) => ledger.reservations.push(reservation),
                None => eprintln!("ob-node: {FILE}: dropped a reservation that could not be read"),
            }
        }
        Ok(ledger)
    }

    fn save(&self) -> Result<(), String> {
        let on_hand: Vec<Value> = self
            .on_hand
            .iter()
            .map(|(key, count)| {
                let (listing, variant) = key_hex(key);
                json!({ "listing": listing, "variant": variant, "count": count })
            })
            .collect();
        let reservations: Vec<Value> = self
            .reservations
            .iter()
            .map(|r| {
                let (listing, variant) = key_hex(&r.key);
                json!({
                    "order": r.order, "listing": listing, "variant": variant,
                    "quantity": r.quantity, "expiresAt": r.expires_at, "state": r.state.as_str(),
                })
            })
            .collect();
        let bytes = serde_json::to_vec(&json!({ "v": 1, "onHand": on_hand, "reservations": reservations }))
            .map_err(|err| err.to_string())?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| err.to_string())
    }

    /// Units on the shelf: what the seller last set, less what has sold since.
    /// `signed` is the stock in the current signed listing, used until the
    /// ledger has seen this key.
    pub(crate) fn shelf(&self, key: &Key, signed: u32) -> u32 {
        self.on_hand.get(key).copied().unwrap_or(signed)
    }

    fn held(&self, key: &Key) -> u32 {
        self.reservations.iter().filter(|r| r.key == *key && r.state == State::Held).map(|r| r.quantity).sum()
    }

    /// What a new order can still reserve, and what the next publish shows.
    pub(crate) fn available(&self, key: &Key, signed: u32) -> u32 {
        self.shelf(key, signed).saturating_sub(self.held(key))
    }

    /// Hold `quantity` units for `order` until `expires_at`. Holding again
    /// for the same order is a no-op.
    pub(crate) fn reserve(&mut self, order: &str, key: Key, quantity: u32, signed: u32, expires_at: u64) -> Result<(), Refused> {
        if self.reservations.iter().any(|r| r.order == order && r.key == key && r.state == State::Held) {
            return Ok(());
        }
        let available = self.available(&key, signed);
        if quantity > available {
            return Err(Refused::Short { available });
        }
        self.on_hand.entry(key).or_insert(signed);
        self.reservations.push(Reservation { order: order.to_string(), key, quantity, expires_at, state: State::Held });
        self.save().map_err(Refused::Failed)
    }

    /// `order` was paid. Its held units are sold; if they were released
    /// meanwhile, they are sold again if still there, else it is oversold.
    pub(crate) fn settle(&mut self, order: &str) -> Result<Vec<(Key, Settled)>, String> {
        let mut out = Vec::new();
        for i in 0..self.reservations.len() {
            if self.reservations[i].order != order {
                continue;
            }
            let Reservation { key, quantity, state, .. } = self.reservations[i].clone();
            let settled = match state {
                State::Held => Settled::Sold,
                State::Released if self.available(&key, 0) >= quantity => Settled::Sold,
                State::Released => Settled::Oversold,
                State::Sold | State::Oversold => continue,
            };
            if settled == Settled::Sold {
                let shelf = self.on_hand.entry(key).or_insert(0);
                *shelf = shelf.saturating_sub(quantity);
            }
            self.reservations[i].state = if settled == Settled::Sold { State::Sold } else { State::Oversold };
            out.push((key, settled));
        }
        self.save()?;
        Ok(out)
    }

    /// Give back the units `order` holds (declined, cancelled, or refunded before payment).
    pub(crate) fn release(&mut self, order: &str) -> Result<Vec<Key>, String> {
        let mut keys = Vec::new();
        for r in self.reservations.iter_mut().filter(|r| r.order == order && r.state == State::Held) {
            r.state = State::Released;
            keys.push(r.key);
        }
        if !keys.is_empty() {
            self.save()?;
        }
        Ok(keys)
    }

    /// Release holds whose payment window passed, and forget settled ones
    /// older than [`KEEP_SETTLED`]. Returns the keys whose availability rose.
    pub(crate) fn sweep(&mut self, now: u64) -> Result<Vec<Key>, String> {
        let mut keys = Vec::new();
        for r in self.reservations.iter_mut().filter(|r| r.state == State::Held && r.expires_at <= now) {
            r.state = State::Released;
            keys.push(r.key);
        }
        let before = self.reservations.len();
        self.reservations.retain(|r| r.state == State::Held || r.expires_at.saturating_add(KEEP_SETTLED) > now);
        if !keys.is_empty() || self.reservations.len() != before {
            self.save()?;
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// The seller counted their shelf: `count` units, before current holds.
    pub(crate) fn set_on_hand(&mut self, key: Key, count: u32) -> Result<(), String> {
        self.on_hand.insert(key, count);
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::TempDir;

    fn ledger() -> (TempDir, StockLedger) {
        let dir = TempDir::new();
        std::fs::create_dir_all(&dir.0).unwrap();
        let ledger = StockLedger::open(&dir.0, Vault::from_key(&[7; 32])).unwrap();
        (dir, ledger)
    }

    const TEE: Key = (ListingId([9; 16]), VariantId([1; 8]));

    #[test]
    fn the_last_unit_goes_to_the_first_letter_read() {
        let (_dir, mut stock) = ledger();
        stock.reserve("a", TEE, 1, 1, 100).unwrap();
        assert_eq!(stock.reserve("b", TEE, 1, 1, 100), Err(Refused::Short { available: 0 }));
        assert_eq!(stock.settle("a").unwrap(), vec![(TEE, Settled::Sold)]);
        assert_eq!(stock.available(&TEE, 1), 0, "the ledger, not the signed stock, is the authority now");
    }

    #[test]
    fn an_unpaid_hold_expires_and_a_late_payment_is_oversold() {
        let (_dir, mut stock) = ledger();
        stock.reserve("a", TEE, 2, 3, 100).unwrap();
        assert_eq!(stock.available(&TEE, 3), 1);
        assert_eq!(stock.sweep(100).unwrap(), vec![TEE]);
        assert_eq!(stock.available(&TEE, 3), 3);
        stock.reserve("b", TEE, 3, 3, 200).unwrap();
        stock.settle("b").unwrap();
        assert_eq!(stock.settle("a").unwrap(), vec![(TEE, Settled::Oversold)]);
        assert_eq!(stock.settle("a").unwrap(), vec![], "settling twice does nothing");
    }

    #[test]
    fn a_late_payment_still_sells_when_units_remain() {
        let (_dir, mut stock) = ledger();
        stock.reserve("a", TEE, 1, 5, 100).unwrap();
        stock.sweep(150).unwrap();
        assert_eq!(stock.settle("a").unwrap(), vec![(TEE, Settled::Sold)]);
        assert_eq!(stock.available(&TEE, 5), 4);
    }

    #[test]
    fn holds_survive_a_restart_and_restocking_counts_against_them() {
        let (dir, mut stock) = ledger();
        stock.reserve("a", TEE, 2, 2, 100).unwrap();
        stock.reserve("a", TEE, 2, 2, 100).unwrap();
        drop(stock);
        let mut stock = StockLedger::open(&dir.0, Vault::from_key(&[7; 32])).unwrap();
        assert_eq!(stock.available(&TEE, 2), 0);
        stock.set_on_hand(TEE, 10).unwrap();
        assert_eq!(stock.available(&TEE, 10), 8);
        assert_eq!(stock.release("a").unwrap(), vec![TEE]);
        assert_eq!(stock.available(&TEE, 10), 10);
    }

    #[test]
    fn settled_reservations_are_forgotten_after_a_month() {
        let (_dir, mut stock) = ledger();
        stock.reserve("a", TEE, 1, 1, 100).unwrap();
        stock.settle("a").unwrap();
        stock.sweep(100 + KEEP_SETTLED).unwrap();
        assert!(stock.reservations.is_empty());
    }
}
