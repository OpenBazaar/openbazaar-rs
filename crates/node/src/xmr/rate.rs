//! The XMR price, in sats, for pricing Monero orders.
//!
//! Coinbase (the BTC fiat source) does not list XMR, so this asks Kraken's
//! public ticker for the last XMR/BTC trade, through the same proxy and
//! `--no-rates` switch as [`crate::rates`]. The answer is an integer number
//! of sats per XMR; order amounts come from it with integer math
//! ([`openbazaar_xmr::amount::piconero_for_sats`]). Fiat for XMR is the BTC
//! fiat price scaled by it, for display only.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use openbazaar_xmr::amount::{fiat_per_xmr, parse_decimal};
use serde_json::{json, Value};

use crate::Egress;

const SOURCE: &str = "https://api.kraken.com/0/public/Ticker?pair=XMRXBT";
const FRESH: Duration = Duration::from_secs(5 * 60);
/// An order is never priced from an answer older than this.
const USABLE: Duration = Duration::from_secs(30 * 60);

pub(crate) struct XmrRate {
    cache: Mutex<Option<(Instant, u64)>>,
    egress: Egress,
    source: String,
}

impl XmrRate {
    pub(crate) fn new(egress: Egress) -> Self {
        Self { cache: Mutex::new(None), egress, source: SOURCE.to_string() }
    }

    /// Sats per XMR, or `None` with `--no-rates` or no usable answer.
    pub(crate) fn sats_per_xmr(&self) -> Option<u64> {
        if !self.egress.live_rates {
            return None;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((at, sats)) = *cache {
            if at.elapsed() < FRESH {
                return Some(sats);
            }
        }
        if let Some(sats) = fetch(&self.egress, &self.source) {
            *cache = Some((Instant::now(), sats));
            return Some(sats);
        }
        cache.filter(|(at, _)| at.elapsed() < USABLE).map(|(_, sats)| sats)
    }

    /// The `xmr` field of `GET /rates`, given the BTC answer.
    pub(crate) fn value(&self, btc: &Value) -> Value {
        let Some(sats) = self.sats_per_xmr() else { return Value::Null };
        let mut currencies = serde_json::Map::new();
        if let Some(map) = btc["currencies"].as_object() {
            for (code, price) in map {
                if let Some(price) = price.as_f64() {
                    currencies.insert(code.clone(), json!(fiat_per_xmr(price, sats)));
                }
            }
        }
        json!({ "base": "XMR", "satsPerXmr": sats, "decimals": 12, "source": "kraken", "currencies": currencies })
    }

    #[cfg(test)]
    pub(crate) fn set_for_tests(&self, sats: Option<u64>) {
        *self.cache.lock().unwrap() = sats.map(|sats| (Instant::now(), sats));
    }
}

fn fetch(egress: &Egress, source: &str) -> Option<u64> {
    let agent = egress.ureq_agent(Duration::from_secs(3), Duration::from_secs(4)).ok()?;
    parse_kraken(&agent.get(source).call().ok()?.into_string().ok()?)
}

/// The last trade price from Kraken's ticker, in sats (8 decimals).
pub(crate) fn parse_kraken(body: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value["error"].as_array().is_some_and(|errors| !errors.is_empty()) {
        return None;
    }
    let (_, pair) = value["result"].as_object()?.iter().next()?;
    parse_decimal(pair["c"][0].as_str()?, 8).filter(|sats| *sats > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kraken_xmr_btc_into_integer_sats() {
        let body = r#"{"error":[],"result":{"XXMRXXBT":{"a":["0.00814300","1","1.000"],"c":["0.00814200","0.5"]}}}"#;
        assert_eq!(parse_kraken(body), Some(814_200));
        assert_eq!(parse_kraken(r#"{"error":["EQuery:Unknown asset pair"]}"#), None);
        assert_eq!(parse_kraken(r#"{"error":[],"result":{"X":{"c":["0.000000001","1"]}}}"#), None, "more than 8 decimals");
        assert_eq!(parse_kraken(r#"{"error":[],"result":{"X":{"c":["0","1"]}}}"#), None);
    }

    #[test]
    fn xmr_rates_go_through_the_proxy_and_stop_with_no_rates() {
        let (proxy, seen) = crate::egress::fake_socks::start();
        let rate = XmrRate::new(Egress { proxy: Some(proxy), live_rates: true });
        assert_eq!(rate.value(&json!({})), Value::Null);
        let (kind, host) = seen.recv_timeout(Duration::from_secs(5)).expect("no request reached the proxy");
        assert_eq!((kind, host.as_str()), (3, "api.kraken.com"));
        let off = XmrRate::new(Egress { proxy: None, live_rates: false });
        assert_eq!(off.sats_per_xmr(), None);
    }

    #[test]
    fn fiat_is_scaled_from_btc() {
        let rate = XmrRate::new(Egress::default());
        rate.set_for_tests(Some(1_000_000));
        let value = rate.value(&json!({"currencies": {"USD": 100000.0}}));
        assert_eq!(value["satsPerXmr"], 1_000_000);
        assert_eq!(value["currencies"]["USD"], 1000.0);
    }
}
