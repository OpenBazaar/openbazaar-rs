//! Amounts in zatoshi (10^-8 ZEC), with integer math only.

pub const ZATOSHI_PER_ZEC: u64 = 100_000_000;
const ZEC_DECIMALS: usize = 8;

/// `1.5` ZEC as text: no trailing zeros, at least one digit.
pub fn format_zec(zatoshi: u64) -> String {
    let whole = zatoshi / ZATOSHI_PER_ZEC;
    let fraction = zatoshi % ZATOSHI_PER_ZEC;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:0ZEC_DECIMALS$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// Zatoshi owed for `sats` at `sats_per_zec` (the price of 1 ZEC in
/// satoshis), rounded up so the vendor is never paid less than the price.
pub fn zatoshi_for_sats(sats: u64, sats_per_zec: u64) -> Option<u64> {
    if sats_per_zec == 0 {
        return None;
    }
    let numerator = u128::from(sats) * u128::from(ZATOSHI_PER_ZEC);
    u64::try_from(numerator.div_ceil(u128::from(sats_per_zec))).ok()
}

/// Satoshis per ZEC from the prices of 1 ZEC and 1 BTC in one fiat currency,
/// rounded to the nearest satoshi. No exchange lists ZEC/BTC on the route the
/// node uses, so the rate is crossed. `None` for a price that is missing,
/// not positive, or gives less than one satoshi.
pub fn sats_per_zec(zec_fiat: f64, btc_fiat: f64) -> Option<u64> {
    if !(zec_fiat.is_finite() && btc_fiat.is_finite() && zec_fiat > 0.0 && btc_fiat > 0.0) {
        return None;
    }
    let sats = (zec_fiat / btc_fiat * ZATOSHI_PER_ZEC as f64).round();
    (sats >= 1.0 && sats < u64::MAX as f64).then_some(sats as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_zatoshi_without_float_math() {
        assert_eq!(format_zec(0), "0");
        assert_eq!(format_zec(1), "0.00000001");
        assert_eq!(format_zec(150_000_000), "1.5");
        assert_eq!(format_zec(10_000), "0.0001");
        assert_eq!(format_zec(u64::MAX), "184467440737.09551615");
    }

    #[test]
    fn converts_sats_to_zatoshi_rounding_up() {
        // 1 ZEC = 0.0123 BTC = 1,230,000 sats.
        assert_eq!(zatoshi_for_sats(1_230_000, 1_230_000), Some(ZATOSHI_PER_ZEC));
        // 18,000 sats / 1,230,000 sats per ZEC = 0.0146341463... ZEC.
        assert_eq!(zatoshi_for_sats(18_000, 1_230_000), Some(1_463_415));
        assert_eq!(zatoshi_for_sats(1, 1_230_000), Some(82));
        assert_eq!(zatoshi_for_sats(1, 0), None);
        assert_eq!(zatoshi_for_sats(u64::MAX, 1), None);
    }

    #[test]
    fn crosses_two_fiat_prices() {
        // ZEC at 1,229.36 USD and BTC at 100,000 USD: 0.0122936 BTC.
        assert_eq!(sats_per_zec(1_229.36, 100_000.0), Some(1_229_360));
        for (zec, btc) in [(0.0, 1.0), (1.0, 0.0), (-1.0, 1.0), (f64::NAN, 1.0), (1.0, f64::INFINITY), (1.0, 1e12)] {
            assert_eq!(sats_per_zec(zec, btc), None, "{zec} / {btc}");
        }
    }
}
