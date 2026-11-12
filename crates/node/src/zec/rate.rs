//! The ZEC price, in sats, for pricing Zcash orders.
//!
//! Coinbase (the BTC fiat source) does not answer for ZEC/BTC and Kraken
//! does not list the pair, so this asks Kraken's public ticker for the last
//! ZEC/USD trade and crosses it with the node's BTC/USD price
//! ([`openbazaar_zec::amount::sats_per_zec`]), through the same proxy and
//! `--no-rates` switch as [`crate::rates`]. With no proxy the request is not
//! made, and a Kraken that fails or refuses is not asked again at once (see
//! [`crate::backoff`]). Order amounts come from the integer sats per ZEC with integer math.
//! Fiat for ZEC is the BTC fiat price scaled by it, for display only.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use openbazaar_zec::amount::{sats_per_zec, ZATOSHI_PER_ZEC};
use serde_json::{json, Value};

use crate::backoff::{asked_wait, for_prices, Backoff};
use crate::Egress;

const SOURCE: &str = "https://api.kraken.com/0/public/Ticker?pair=ZECUSD";
const FRESH: Duration = Duration::from_secs(5 * 60);
/// An order is never priced from an answer older than this, for ZEC/USD or for the BTC/USD it is crossed with.
pub(crate) const USABLE: Duration = Duration::from_secs(30 * 60);

pub(crate) struct ZecRate {
    /// The last ZEC/USD answer.
    cache: Mutex<Option<(Instant, f64)>>,
    retry: Mutex<Backoff>,
    egress: Egress,
    source: String,
    /// Tests and `--fixed-rates` pin sats per ZEC instead of asking Kraken and Coinbase.
    pub(crate) fixed: Mutex<Option<u64>>,
}

impl ZecRate {
    pub(crate) fn new(egress: Egress) -> Self {
        Self {
            cache: Mutex::new(None),
            retry: Mutex::new(for_prices()),
            egress,
            source: SOURCE.to_string(),
            fixed: Mutex::new(None),
        }
    }

    /// USD per ZEC, or `None` with `--no-rates` or no usable answer.
    fn usd_per_zec(&self) -> Option<f64> {
        if !self.egress.live_rates {
            return None;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((at, usd)) = *cache {
            if at.elapsed() < FRESH {
                return Some(usd);
            }
        }
        let mut retry = self.retry.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(usd) = retry.ask(|| fetch(&self.egress, &self.source)) {
            *cache = Some((Instant::now(), usd));
            return Some(usd);
        }
        cache
            .filter(|(at, _)| at.elapsed() < USABLE)
            .map(|(_, usd)| usd)
    }

    /// Sats per ZEC given the node's BTC/USD price, or `None` without either price.
    pub(crate) fn sats_per_zec(&self, btc_usd: Option<f64>) -> Option<u64> {
        if let Some(sats) = *self.fixed.lock().unwrap_or_else(|err| err.into_inner()) {
            return Some(sats);
        }
        // Without a BTC/USD price there is nothing to cross, so Kraken is not asked.
        let btc_usd = btc_usd?;
        sats_per_zec(self.usd_per_zec()?, btc_usd)
    }

    /// The `zec` field of `GET /rates`, given the BTC answer.
    pub(crate) fn value(&self, btc: &Value) -> Value {
        let Some(sats) = self.sats_per_zec(btc["currencies"]["USD"].as_f64()) else {
            return Value::Null;
        };
        let mut currencies = serde_json::Map::new();
        if let Some(map) = btc["currencies"].as_object() {
            for (code, price) in map {
                if let Some(price) = price.as_f64() {
                    currencies.insert(
                        code.clone(),
                        json!(price * sats as f64 / ZATOSHI_PER_ZEC as f64),
                    );
                }
            }
        }
        let source = if self
            .fixed
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_some()
        {
            "fixed"
        } else {
            "kraken"
        };
        json!({ "base": "ZEC", "satsPerZec": sats, "decimals": 8, "source": source, "currencies": currencies })
    }

    #[cfg(test)]
    pub(crate) fn set_for_tests(&self, sats: Option<u64>) {
        *self.fixed.lock().unwrap() = sats;
    }

    /// Tests keep a fresh ZEC/USD answer and leave the BTC/USD leg live.
    #[cfg(test)]
    pub(crate) fn set_usd_for_tests(&self, usd: f64) {
        *self.cache.lock().unwrap() = Some((Instant::now(), usd));
    }
}

/// The error is the wait Kraken asked for, if it named or earned one.
fn fetch(egress: &Egress, source: &str) -> Result<f64, Option<Duration>> {
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
    parse_kraken_usd(&body).ok_or(None)
}

/// The last trade price from Kraken's ZEC/USD ticker.
pub(crate) fn parse_kraken_usd(body: &str) -> Option<f64> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value["error"]
        .as_array()
        .is_some_and(|errors| !errors.is_empty())
    {
        return None;
    }
    let (_, pair) = value["result"].as_object()?.iter().next()?;
    pair["c"][0]
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|usd| usd.is_finite() && *usd > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kraken_zec_usd() {
        let body = r#"{"error":[],"result":{"XZECZUSD":{"a":["1229.40000","1","1.000"],"c":["1229.36000","0.5"]}}}"#;
        assert_eq!(parse_kraken_usd(body), Some(1229.36));
        assert_eq!(
            parse_kraken_usd(r#"{"error":["EQuery:Unknown asset pair"]}"#),
            None
        );
        assert_eq!(
            parse_kraken_usd(r#"{"error":[],"result":{"X":{"c":["0","1"]}}}"#),
            None
        );
        assert_eq!(
            parse_kraken_usd(r#"{"error":[],"result":{"X":{"c":["-1","1"]}}}"#),
            None
        );
        assert_eq!(parse_kraken_usd(r#"{"error":[],"result":{}}"#), None);
    }

    #[test]
    fn zec_rates_go_through_the_proxy_and_stop_with_no_rates() {
        let (proxy, seen) = crate::egress::fake_socks::start();
        let rate = ZecRate::new(Egress {
            proxy: Some(proxy),
            live_rates: true,
        });
        assert_eq!(
            rate.value(&json!({"currencies": {"USD": 100000.0}})),
            Value::Null
        );
        let (kind, host) = seen
            .recv_timeout(Duration::from_secs(5))
            .expect("no request reached the proxy");
        assert_eq!((kind, host.as_str()), (3, "api.kraken.com"));
        let off = ZecRate::new(Egress {
            proxy: None,
            live_rates: false,
        });
        assert_eq!(off.sats_per_zec(Some(100000.0)), None);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut clear = ZecRate::new(Egress {
            proxy: None,
            live_rates: true,
        });
        clear.source = format!("http://{}/", listener.local_addr().unwrap());
        assert_eq!(clear.sats_per_zec(Some(100000.0)), None);
        assert!(
            listener.accept().is_err(),
            "Kraken was dialed without a proxy"
        );
    }

    #[test]
    fn a_kraken_that_refuses_is_asked_once_and_never_without_a_btc_price() {
        let (proxy, asked) = crate::egress::fake_socks::start_http(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let mut rate = ZecRate::new(Egress {
            proxy: Some(proxy),
            live_rates: true,
        });
        rate.source = "http://rates.test/".into();
        for _ in 0..14 {
            assert_eq!(rate.sats_per_zec(None), None);
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "there was no BTC price to cross with"
        );
        for _ in 0..14 {
            assert_eq!(rate.sats_per_zec(Some(100_000.0)), None);
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "14 requests for a price while Kraken refuses"
        );
    }

    #[test]
    fn the_rate_is_crossed_with_btc_and_needs_both_prices() {
        let rate = ZecRate::new(Egress::default());
        *rate.cache.lock().unwrap() = Some((Instant::now(), 1229.36));
        assert_eq!(rate.sats_per_zec(None), None, "no BTC/USD, no ZEC price");
        let rate = ZecRate {
            egress: Egress {
                proxy: None,
                live_rates: true,
            },
            ..rate
        };
        assert_eq!(rate.sats_per_zec(Some(100_000.0)), Some(1_229_360));
        let value = rate.value(&json!({"currencies": {"USD": 100_000.0, "EUR": 90_000.0}}));
        assert_eq!(value["satsPerZec"], 1_229_360);
        assert_eq!(value["currencies"]["USD"], 1229.36);
        assert_eq!(value["currencies"]["EUR"], 1106.424);
    }
}
