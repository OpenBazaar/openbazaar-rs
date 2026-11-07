//! The own store's coupon codes, signed with the store key.
//!
//! Kept in `coupons.obv`, sealed with the vault key. A withdrawn coupon is
//! dropped, so orders that name its code are declined from then on. How
//! often each code was used lives in the order-terms ledger.

use std::path::{Path, PathBuf};

use openbazaar_protocol::{Coupon, Discount, SignedCoupon};
use serde_json::{json, Value};

use crate::escrow_mail::{hex, unhex};
use crate::own_listings::{capitalize, parse_id, StoreError};
use crate::vault::Vault;
use crate::{Error, Node};

const FILE: &str = "coupons.obv";
const MAX_COUPONS: usize = 200;

pub(crate) struct CouponBook {
    path: PathBuf,
    vault: Vault,
    items: Vec<SignedCoupon>,
}

impl CouponBook {
    pub(crate) fn open(dir: &Path, vault: Vault) -> Result<Self, Error> {
        let path = dir.join(FILE);
        let mut items = Vec::new();
        if let Some(bytes) = vault.read_sealed(&path)? {
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Detail(format!("{FILE} could not be read")))?;
            for record in value["coupons"].as_array().into_iter().flatten() {
                match record.as_str().and_then(|text| unhex(text).ok()).and_then(|bytes| SignedCoupon::decode(&bytes).ok()) {
                    Some(coupon) => items.push(coupon),
                    None => eprintln!("ob-node: {FILE}: dropped a coupon that could not be read"),
                }
            }
        }
        Ok(Self { path, vault, items })
    }

    fn save(&self) -> Result<(), StoreError> {
        let records: Result<Vec<String>, _> = self.items.iter().map(|c| c.encode().map(|bytes| hex(&bytes))).collect();
        let records = records.map_err(|err| StoreError::Failed(err.to_string()))?;
        let bytes = serde_json::to_vec(&json!({ "v": 1, "coupons": records })).map_err(|err| StoreError::Failed(err.to_string()))?;
        self.vault.write_sealed(&self.path, &bytes).map_err(|err| StoreError::Failed(err.to_string()))
    }

    pub(crate) fn all(&self) -> &[SignedCoupon] {
        &self.items
    }
}

fn coupon_value(coupon: &Coupon, uses: u32) -> Value {
    let (percent, fixed) = match coupon.discount {
        Discount::Percent(bps) => (Some(bps), None),
        Discount::Fixed(minor) => (None, Some(minor)),
    };
    json!({
        "code": coupon.code,
        "percentBps": percent,
        "fixed": fixed,
        "currency": coupon.currency,
        "minSubtotal": coupon.min_subtotal,
        "listings": coupon.listings.iter().map(|id| id.to_hex()).collect::<Vec<_>>(),
        "startsOn": coupon.starts_on,
        "endsOn": coupon.ends_on,
        "maxUses": coupon.max_uses,
        "uses": uses,
    })
}

fn number(body: &Value, key: &str, max: u64) -> Result<u64, StoreError> {
    match &body[key] {
        Value::Null => Ok(0),
        value => value.as_u64().filter(|n| *n <= max).ok_or_else(|| StoreError::Invalid(format!("The {key} is not a valid number."))),
    }
}

impl Node {
    pub(crate) fn coupons(&self) -> std::sync::MutexGuard<'_, CouponBook> {
        self.coupons.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// `GET /coupons`.
    pub(crate) fn own_coupons_value(&self) -> Value {
        let terms = self.order_terms();
        let items: Vec<Value> = self.coupons().all().iter().map(|c| coupon_value(&c.terms, terms.coupon_uses(&c.terms.code))).collect();
        json!({ "coupons": items })
    }

    /// `POST /coupons`: sign a new code, or a new version of an existing one.
    pub(crate) fn create_coupon(&self, body: &Value) -> Result<Value, StoreError> {
        let secret = self.own_signing_key().ok_or(StoreError::Locked(self.lock.state().as_str()))?;
        let discount = match (&body["percentBps"], &body["fixed"]) {
            (Value::Null, Value::Null) => return Err(StoreError::Invalid("Choose a percentage or an amount off.".into())),
            (Value::Null, _) => Discount::Fixed(number(body, "fixed", u64::MAX)?),
            (_, Value::Null) => Discount::Percent(number(body, "percentBps", 10_000)? as u16),
            _ => return Err(StoreError::Invalid("Choose a percentage or an amount off, not both.".into())),
        };
        let mut listings = Vec::new();
        for id in body["listings"].as_array().into_iter().flatten() {
            let id = id.as_str().and_then(parse_id).ok_or_else(|| StoreError::Invalid("A listing in the coupon is not in your store.".into()))?;
            if self.own_listings().by_id(&id).is_none() {
                return Err(StoreError::Invalid("A listing in the coupon is not in your store.".into()));
            }
            listings.push(id);
        }
        listings.sort();
        listings.dedup();
        let coupon = Coupon {
            store_id: self.own_store,
            code: Coupon::normalize(body["code"].as_str().unwrap_or("")),
            discount,
            currency: body["currency"].as_str().unwrap_or("").to_string(),
            min_subtotal: number(body, "minSubtotal", u64::MAX)?,
            listings,
            starts_on: number(body, "startsOn", 99_991_231)? as u32,
            ends_on: number(body, "endsOn", 99_991_231)? as u32,
            max_uses: number(body, "maxUses", u64::from(u32::MAX))? as u32,
        };
        let signed = SignedCoupon::sign(coupon, &secret).map_err(|err| StoreError::Invalid(capitalize(&err.to_string())))?;
        let mut book = self.coupons();
        let before = book.items.clone();
        book.items.retain(|c| c.terms.code != signed.terms.code);
        if book.items.len() >= MAX_COUPONS {
            book.items = before;
            return Err(StoreError::Invalid(format!("A store can have up to {MAX_COUPONS} coupons.")));
        }
        book.items.push(signed.clone());
        if let Err(err) = book.save() {
            book.items = before;
            return Err(err);
        }
        drop(book);
        Ok(coupon_value(&signed.terms, self.order_terms().coupon_uses(&signed.terms.code)))
    }

    /// `DELETE /coupons/{code}`.
    pub(crate) fn withdraw_coupon(&self, code: &str) -> Result<(), StoreError> {
        let code = Coupon::normalize(code);
        let mut book = self.coupons();
        let before = book.items.clone();
        book.items.retain(|c| c.terms.code != code);
        if book.items.len() == before.len() {
            return Err(StoreError::NotFound);
        }
        if let Err(err) = book.save() {
            book.items = before;
            return Err(err);
        }
        Ok(())
    }
}
