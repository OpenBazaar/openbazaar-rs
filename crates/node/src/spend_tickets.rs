//! Step-up for every spend: quotes and one-time spend tickets.
//!
//! `POST /orders/{id}/pay/quote` fixes the exact terms of one payment or
//! escrow funding: order id, amount, recipient script, and a fee cap. The
//! UI shows those values; the user confirms them with the passphrase at
//! `POST /spend-tickets`, which turns the quote into a ticket. The ticket
//! is redeemed once at `POST /orders/{id}/pay`, for that order only, and
//! the pay worker refuses a transaction that differs from its terms.
//!
//! Quotes and tickets live in memory for [`TICKET_TTL`] and are dropped on lock.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bitcoin::ScriptBuf;
use serde_json::{json, Value};

use crate::vault_key::{encode_hex, random_bytes};

pub(crate) const TICKET_TTL: Duration = Duration::from_secs(60);
/// The fee cap is the quoted fee plus half.
pub(crate) const FEE_CAP_PERCENT: u64 = 150;
const MAX_OPEN: usize = 64;

/// What one spend is allowed to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpendTerms {
    pub order_id: String,
    pub amount_sats: u64,
    pub fee_cap_sats: u64,
    pub recipient: ScriptBuf,
    /// Shown to the user; not enforced (the fee cap is).
    pub address: String,
    pub fee_rate_sat_vb: f64,
}

impl SpendTerms {
    pub(crate) fn value(&self, network: &str) -> Value {
        json!({
            "orderId": self.order_id,
            "network": network,
            "amountSats": self.amount_sats,
            "feeCapSats": self.fee_cap_sats,
            "address": self.address,
            "recipientScript": encode_hex(self.recipient.as_bytes()),
            "feeRateSatVb": self.fee_rate_sat_vb,
            "expiresInSecs": TICKET_TTL.as_secs(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TicketError {
    /// Unknown, used, expired, or dropped by a lock.
    Unknown,
    /// A ticket for another order.
    OtherOrder,
}

struct Open {
    terms: SpendTerms,
    expires: Instant,
}

#[derive(Default)]
pub(crate) struct SpendTickets {
    quotes: Mutex<HashMap<String, Open>>,
    tickets: Mutex<HashMap<String, Open>>,
}

fn guard<T>(map: &Mutex<T>) -> MutexGuard<'_, T> {
    map.lock().unwrap_or_else(|err| err.into_inner())
}

fn insert(map: &Mutex<HashMap<String, Open>>, terms: SpendTerms, now: Instant) -> String {
    let mut map = guard(map);
    map.retain(|_, open| open.expires > now);
    if map.len() >= MAX_OPEN {
        if let Some(oldest) = map.iter().min_by_key(|(_, open)| open.expires).map(|(id, _)| id.clone()) {
            map.remove(&oldest);
        }
    }
    let id = encode_hex(&random_bytes::<32>());
    map.insert(id.clone(), Open { terms, expires: now + TICKET_TTL });
    id
}

/// Remove `id` from `map`: it is good for one use whether or not that use succeeds.
fn take(map: &Mutex<HashMap<String, Open>>, id: &str, now: Instant) -> Option<SpendTerms> {
    let open = guard(map).remove(id)?;
    (open.expires > now).then_some(open.terms)
}

impl SpendTickets {
    pub(crate) fn quote(&self, terms: SpendTerms, now: Instant) -> String {
        insert(&self.quotes, terms, now)
    }

    /// The user confirmed `quote`. Returns the ticket and its terms.
    pub(crate) fn issue(&self, quote: &str, now: Instant) -> Result<(String, SpendTerms), TicketError> {
        let terms = take(&self.quotes, quote, now).ok_or(TicketError::Unknown)?;
        Ok((insert(&self.tickets, terms.clone(), now), terms))
    }

    /// Spend `ticket` on `order_id`. A ticket is gone after this call, even
    /// when it named another order.
    pub(crate) fn redeem(&self, ticket: &str, order_id: &str, now: Instant) -> Result<SpendTerms, TicketError> {
        let terms = take(&self.tickets, ticket, now).ok_or(TicketError::Unknown)?;
        if terms.order_id != order_id {
            return Err(TicketError::OtherOrder);
        }
        Ok(terms)
    }

    pub(crate) fn clear(&self) {
        guard(&self.quotes).clear();
        guard(&self.tickets).clear();
    }
}
