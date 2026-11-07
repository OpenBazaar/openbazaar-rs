//! BTC prices for the shop's fiat estimate.
//!
//! The figure is not a payment rail. The node asks Coinbase's public exchange
//! rates once for USD, EUR, GBP, and JPY, and keeps the answer for several
//! minutes. The shop never dials an exchange. If Coinbase is down and there
//! is no cached answer, the response still succeeds with no currencies, and
//! the page shows the bitcoin amount alone.
//!
//! The request goes through the configured proxy (see [`crate::egress`]), and
//! with `--no-rates` it is never made.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::egress::Egress;

const SOURCE: &str = "https://api.coinbase.com/v2/exchange-rates?currency=BTC";
const FRESH: Duration = Duration::from_secs(5 * 60);
const CODES: [&str; 4] = ["USD", "EUR", "GBP", "JPY"];

struct Cached {
    at: Instant,
    currencies: BTreeMap<String, f64>,
}

pub(crate) struct Rates {
    cache: Mutex<Option<Cached>>,
    egress: Egress,
    source: String,
    /// Tests pin the rates instead of asking Coinbase.
    #[cfg(test)]
    pub(crate) fixed: Mutex<Option<BTreeMap<String, f64>>>,
}

impl Rates {
    pub(crate) fn new(egress: Egress) -> Self {
        Self {
            cache: Mutex::new(None),
            egress,
            source: SOURCE.to_string(),
            #[cfg(test)]
            fixed: Mutex::new(None),
        }
    }

    /// BTC priced in `currency`, if the node has a rate for it.
    pub(crate) fn btc_price(&self, currency: &str) -> Option<f64> {
        self.value()["currencies"][currency].as_f64().filter(|price| price.is_finite() && *price > 0.0)
    }

    pub(crate) fn value(&self) -> Value {
        #[cfg(test)]
        if let Some(fixed) = self.fixed.lock().unwrap_or_else(|err| err.into_inner()).as_ref() {
            return rates_json(fixed);
        }
        if !self.egress.live_rates {
            return json!({ "base": "BTC", "currencies": {}, "live": false });
        }
        let mut guard = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(cached) = guard.as_ref() {
            if cached.at.elapsed() < FRESH {
                return rates_json(&cached.currencies);
            }
        }
        match fetch_coinbase(&self.egress, &self.source) {
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

fn fetch_coinbase(egress: &Egress, source: &str) -> Result<BTreeMap<String, f64>, ()> {
    let agent = egress
        .ureq_agent(Duration::from_secs(3), Duration::from_secs(4))
        .map_err(|_| ())?;
    let body = agent
        .get(source)
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
    fn rates_go_through_the_proxy_with_the_name_unresolved() {
        let (proxy, seen) = crate::egress::fake_socks::start();
        let rates = Rates::new(Egress { proxy: Some(proxy), live_rates: true });
        assert_eq!(rates.value()["currencies"], json!({}));
        let (kind, host) = seen.recv_timeout(Duration::from_secs(5)).expect("no request reached the proxy");
        assert_eq!((kind, host.as_str()), (3, "api.coinbase.com"));
    }

    #[test]
    fn no_rates_means_no_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut rates = Rates::new(Egress { proxy: None, live_rates: false });
        rates.source = format!("http://{}/", listener.local_addr().unwrap());
        assert_eq!(rates.value()["live"], false);
        assert!(listener.accept().is_err(), "the node dialed the rates server");
    }

    #[test]
    fn rejects_a_payload_without_a_usable_price() {
        assert!(parse_coinbase(r#"{"data":{"rates":{"USD":"0"}}}"#).is_err());
        assert!(parse_coinbase("not json").is_err());
    }
}
