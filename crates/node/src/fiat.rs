//! Fiat prices on version 6 listings, converted to sats.
//!
//! A listing priced in a fiat currency also signs a sats price: that amount
//! at the rate on the day it was signed. When a rate is at hand the node
//! converts the fiat price again, so the seller is paid what they asked for;
//! without one it falls back to the signed sats price.

use openbazaar_protocol::{FiatPrice, Listing};

use crate::Node;

/// Currencies the node has rates for.
pub(crate) const CURRENCIES: [&str; 4] = ["USD", "EUR", "GBP", "JPY"];

/// `minor` units of `currency` in sats at `btc_price` (units of the currency per BTC).
pub(crate) fn sats_for(minor: u64, currency: &str, btc_price: f64) -> Option<u64> {
    if !(btc_price.is_finite() && btc_price > 0.0) {
        return None;
    }
    let major = minor as f64 / FiatPrice::scale(currency) as f64;
    let sats = (major / btc_price * 100_000_000.0).round();
    (sats.is_finite() && sats >= 1.0 && sats < u64::MAX as f64).then_some(sats as u64)
}

/// A listing's item price and shipping in sats, at `btc_price` if it is priced in fiat.
pub(crate) fn listing_sats(listing: &Listing, btc_price: impl Fn(&str) -> Option<f64>) -> (u64, u64) {
    let fiat = listing.details.as_ref().and_then(|details| details.price.as_ref());
    match fiat.and_then(|price| btc_price(&price.currency).map(|rate| (price, rate))) {
        Some((price, rate)) => (
            sats_for(price.amount, &price.currency, rate).unwrap_or(listing.price_sats),
            if price.shipping == 0 { 0 } else { sats_for(price.shipping, &price.currency, rate).unwrap_or(listing.shipping_sats) },
        ),
        None => (listing.price_sats, listing.shipping_sats),
    }
}

impl Node {
    /// BTC priced in `currency`, from the node's rates.
    pub(crate) fn btc_price(&self, currency: &str) -> Option<f64> {
        self.rates.btc_price(currency)
    }

    /// `listing` with its sats price and shipping converted at today's rate.
    pub(crate) fn priced_now(&self, listing: &Listing) -> Listing {
        let (price_sats, shipping_sats) = listing_sats(listing, |code| self.btc_price(code));
        Listing { price_sats, shipping_sats, ..listing.clone() }
    }

    #[cfg(test)]
    pub(crate) fn fix_rates(&self, rates: &[(&str, f64)]) {
        *self.rates.fixed.lock().unwrap() = Some(rates.iter().map(|(code, price)| (code.to_string(), *price)).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_units_convert_at_the_rate() {
        assert_eq!(sats_for(2_500, "USD", 100_000.0), Some(25_000));
        assert_eq!(sats_for(150, "JPY", 15_000_000.0), Some(1_000));
        assert_eq!(sats_for(1, "USD", 0.0), None);
        assert_eq!(sats_for(0, "USD", 100_000.0), None);
    }
}
