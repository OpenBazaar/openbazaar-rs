//! BTC prices for the shop's fiat estimate.
//!
//! The figure is not a payment rail. The node asks Coinbase's public exchange
//! rates once for USD, EUR, GBP, and JPY, and keeps the answer for several
//! minutes. The shop never dials an exchange. If Coinbase is down and there
//! is no cached answer, the response still succeeds with no currencies, and
//! the page shows the bitcoin amount alone.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const SOURCE: &str = "https://api.coinbase.com/v2/exchange-rates?currency=BTC";
const FRESH: Duration = Duration::from_secs(5 * 60);
const CODES: [&str; 4] = ["USD", "EUR", "GBP", "JPY"];

struct Cached {
    at: Instant,
    currencies: BTreeMap<String, f64>,
}

pub(crate) struct Rates {
    cache: Mutex<Option<Cached>>,
}

impl Rates {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(None),
        }
    }

    pub(crate) fn value(&self) -> Value {
        let mut guard = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = guard.as_ref() {
            if cached.at.elapsed() < FRESH {
                return rates_json(&cached.currencies);
            }
        }
        match fetch_coinbase() {
            Ok(currencies) => {
                *guard = Some(Cached {
                    at: Instant::now(),
                    currencies: currencies.clone(),
                });
                rates_json(&currencies)
            }
            Err(_) => match guard.as_ref() {
                Some(cached) => rates_json(&cached.currencies),
                None => json!({ "base": "BTC", "currencies": {} }),
            },
        }
    }
}

fn rates_json(currencies: &BTreeMap<String, f64>) -> Value {
    let mut map = serde_json::Map::new();
    for code in CODES {
        if let Some(price) = currencies.get(code) {
            map.insert((*code).to_string(), json!(price));
        }
    }
    json!({ "base": "BTC", "currencies": map })
}

fn fetch_coinbase() -> Result<BTreeMap<String, f64>, ()> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(4))
        .build();
    let body = agent
        .get(SOURCE)
        .call()
        .map_err(|_| ())?
        .into_string()
        .map_err(|_| ())?;
    parse_coinbase(&body)
}

pub(crate) fn parse_coinbase(body: &str) -> Result<BTreeMap<String, f64>, ()> {
    let value: Value = serde_json::from_str(body).map_err(|_| ())?;
    let rates = value
        .get("data")
        .and_then(|data| data.get("rates"))
        .and_then(Value::as_object)
        .ok_or(())?;
    let mut out = BTreeMap::new();
    for code in CODES {
        let Some(text) = rates.get(code).and_then(Value::as_str) else {
            continue;
        };
        let price: f64 = text.parse().map_err(|_| ())?;
        if price.is_finite() && price > 0.0 {
            out.insert(code.to_string(), price);
        }
    }
    if out.is_empty() {
        return Err(());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_coinbase_payload_into_the_shop_currencies() {
        let body = r#"{
            "data": {
                "currency": "BTC",
                "rates": {
                    "USD": "100250.5",
                    "EUR": "92100",
                    "GBP": "78440.25",
                    "JPY": "15100000",
                    "CAD": "130000"
                }
            }
        }"#;
        let rates = parse_coinbase(body).unwrap();
        assert_eq!(rates.get("USD"), Some(&100250.5));
        assert_eq!(rates.get("EUR"), Some(&92100.0));
        assert_eq!(rates.get("GBP"), Some(&78440.25));
        assert_eq!(rates.get("JPY"), Some(&15100000.0));
        assert!(!rates.contains_key("CAD"));
    }

    #[test]
    fn rejects_a_payload_without_a_usable_price() {
        assert!(parse_coinbase(r#"{"data":{"rates":{"USD":"0"}}}"#).is_err());
        assert!(parse_coinbase("not json").is_err());
    }
}
