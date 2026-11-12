//! BTC prices for the shop's fiat estimate.
//!
//! The figure is not a payment rail. The node asks Coinbase's public exchange
//! rates once for USD, EUR, GBP, and JPY, and keeps the answer for several
//! minutes. The shop never dials an exchange. If Coinbase is down and there
//! is no cached answer, the response still succeeds with no currencies, and
//! the page shows the bitcoin amount alone. A Coinbase that fails or refuses
//! is not asked again at once (see [`crate::backoff`]).
//!
//! The request goes through the configured proxy (see [`crate::egress`]).
//! With `--no-rates`, or with no proxy, it is never made: a direct connection
//! would show this machine's IP to Coinbase.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::backoff::{asked_wait, for_prices, Backoff};
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
    retry: Mutex<Backoff>,
    egress: Egress,
    source: String,
    /// Tests and `--fixed-rates` pin the rates instead of asking Coinbase.
    pub(crate) fixed: Mutex<Option<BTreeMap<String, f64>>>,
}

impl Rates {
    pub(crate) fn new(egress: Egress) -> Self {
        Self {
            cache: Mutex::new(None),
            retry: Mutex::new(for_prices()),
            egress,
            source: SOURCE.to_string(),
            fixed: Mutex::new(None),
        }
    }

    /// BTC priced in `currency`, if the node has a rate for it.
    pub(crate) fn btc_price(&self, currency: &str) -> Option<f64> {
        self.value()["currencies"][currency]
            .as_f64()
            .filter(|price| price.is_finite() && *price > 0.0)
    }

    /// `true` while the last Coinbase answer is younger than `limit`. When
    /// Coinbase fails, `value` goes on serving the old answer at any age.
    pub(crate) fn answered_within(&self, limit: Duration) -> bool {
        if self
            .fixed
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_some()
        {
            return true;
        }
        let guard = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        guard
            .as_ref()
            .is_some_and(|cached| cached.at.elapsed() < limit)
    }

    /// Tests keep a BTC/USD answer of this age, as if Coinbase had failed since.
    #[cfg(test)]
    pub(crate) fn set_answer_for_tests(&self, usd: f64, age: Duration) {
        *self.cache.lock().unwrap() = Some(Cached {
            at: Instant::now() - age,
            currencies: BTreeMap::from([("USD".to_string(), usd)]),
        });
    }

    pub(crate) fn value(&self) -> Value {
        if let Some(fixed) = self
            .fixed
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_ref()
        {
            let mut value = rates_json(fixed);
            value["source"] = json!("fixed");
            return value;
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
        let mut retry = self.retry.lock().unwrap_or_else(|err| err.into_inner());
        match retry.ask(|| fetch_coinbase(&self.egress, &self.source)) {
            Some(currencies) => {
                *guard = Some(Cached {
                    at: Instant::now(),
                    currencies: currencies.clone(),
                });
                rates_json(&currencies)
            }
            None => match guard.as_ref() {
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

/// The error is the wait Coinbase asked for, if it named or earned one.
fn fetch_coinbase(
    egress: &Egress,
    source: &str,
) -> Result<BTreeMap<String, f64>, Option<Duration>> {
    if egress.proxy.is_none() {
        return Err(None);
    }
    let agent = egress
        .ureq_agent(Duration::from_secs(3), Duration::from_secs(4))
        .map_err(|_| None)?;
    let body = agent
        .get(source)
        .call()
        .map_err(|err| asked_wait(&err))?
        .into_string()
        .map_err(|_| None)?;
    parse_coinbase(&body).map_err(|_| None)
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
        let rates = Rates::new(Egress {
            proxy: Some(proxy),
            live_rates: true,
        });
        assert_eq!(rates.value()["currencies"], json!({}));
        let (kind, host) = seen
            .recv_timeout(Duration::from_secs(5))
            .expect("no request reached the proxy");
        assert_eq!((kind, host.as_str()), (3, "api.coinbase.com"));
    }

    /// A node asked for prices `calls` times in a row, behind a proxy whose
    /// rate server answers `answer`: how often it was asked, and the node.
    fn asked_in_a_burst(answer: &'static str, calls: usize) -> (usize, Rates) {
        let (proxy, asked) = crate::egress::fake_socks::start_http(answer);
        let mut rates = Rates::new(Egress {
            proxy: Some(proxy),
            live_rates: true,
        });
        rates.source = "http://rates.test/".into();
        for _ in 0..calls {
            assert_eq!(rates.value()["currencies"], json!({}));
        }
        (asked.load(std::sync::atomic::Ordering::SeqCst), rates)
    }

    /// `true` when the rate server is left alone for `minutes` and asked again a minute later.
    fn left_alone_for(rates: &Rates, minutes: u64) -> bool {
        let retry = rates.retry.lock().unwrap();
        let later = |minutes| Instant::now() + Duration::from_secs(minutes * 60);
        !retry.ready(later(minutes - 1)) && retry.ready(later(minutes))
    }

    #[test]
    fn a_rate_server_that_refuses_is_asked_once_and_left_alone() {
        let (asked, rates) = asked_in_a_burst(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            14,
        );
        assert_eq!(asked, 1, "14 requests for a price while Coinbase refuses");
        assert!(left_alone_for(&rates, 15));
    }

    #[test]
    fn a_rate_server_that_says_when_to_come_back_is_obeyed() {
        let (asked, rates) =
            asked_in_a_burst("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1800\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", 14);
        assert_eq!(
            asked, 1,
            "14 requests for a price while Coinbase says to slow down"
        );
        assert!(left_alone_for(&rates, 30));
    }

    #[test]
    fn a_rate_server_that_fails_is_tried_again_soon_but_not_at_once() {
        let (asked, rates) = asked_in_a_burst(
            "HTTP/1.1 500 No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            14,
        );
        assert_eq!(asked, 1, "14 requests for a price while Coinbase fails");
        assert!(left_alone_for(&rates, 1));
    }

    #[test]
    fn rates_are_not_fetched_in_the_clear() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut rates = Rates::new(Egress {
            proxy: None,
            live_rates: true,
        });
        rates.source = format!("http://{}/", listener.local_addr().unwrap());
        let started = Instant::now();
        assert!(rates.value()["currencies"].as_object().unwrap().is_empty());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(
            listener.accept().is_err(),
            "Coinbase was dialed without a proxy"
        );
    }

    #[test]
    fn no_rates_means_no_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut rates = Rates::new(Egress {
            proxy: None,
            live_rates: false,
        });
        rates.source = format!("http://{}/", listener.local_addr().unwrap());
        assert_eq!(rates.value()["live"], false);
        assert!(
            listener.accept().is_err(),
            "the node dialed the rates server"
        );
    }

    #[test]
    fn rejects_a_payload_without_a_usable_price() {
        assert!(parse_coinbase(r#"{"data":{"rates":{"USD":"0"}}}"#).is_err());
        assert!(parse_coinbase("not json").is_err());
    }
}
