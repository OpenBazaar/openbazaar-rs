//! Amounts in piconero (10^-12 XMR), with integer math only.

pub const PICONERO_PER_XMR: u64 = 1_000_000_000_000;
const SATS_PER_BTC: u128 = 100_000_000;
const XMR_DECIMALS: usize = 12;

/// `1.5` XMR as text: no trailing zeros, at least one digit.
pub fn format_xmr(piconero: u64) -> String {
    let whole = piconero / PICONERO_PER_XMR;
    let fraction = piconero % PICONERO_PER_XMR;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:0XMR_DECIMALS$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// A non-negative decimal like `0.00814200`, scaled by 10^`decimals`.
/// Refuses more fraction digits than `decimals`, signs, exponents, and overflow.
pub fn parse_decimal(text: &str, decimals: u32) -> Option<u64> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|b| b.is_ascii_digit()) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let fraction = fraction.trim_end_matches('0');
    if fraction.len() > decimals as usize {
        return None;
    }
    let scale = 10u64.checked_pow(decimals)?;
    let whole: u64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let fraction_value: u64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().ok()? * 10u64.pow(decimals - fraction.len() as u32)
    };
    whole.checked_mul(scale)?.checked_add(fraction_value)
}

/// Piconero owed for `sats` at `sats_per_xmr` (the price of 1 XMR in
/// satoshis), rounded up so the vendor is never paid less than the price.
pub fn piconero_for_sats(sats: u64, sats_per_xmr: u64) -> Option<u64> {
    if sats_per_xmr == 0 {
        return None;
    }
    let numerator = u128::from(sats) * u128::from(PICONERO_PER_XMR);
    let piconero = numerator.div_ceil(u128::from(sats_per_xmr));
    u64::try_from(piconero).ok()
}

/// The fiat price of 1 XMR in `fiat_per_btc` units, from satoshis per XMR.
/// Only for display next to a price; the amount owed never comes from here.
pub fn fiat_per_xmr(fiat_per_btc: f64, sats_per_xmr: u64) -> f64 {
    fiat_per_btc * (sats_per_xmr as f64) / (SATS_PER_BTC as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_piconero_without_float_math() {
        assert_eq!(format_xmr(0), "0");
        assert_eq!(format_xmr(1), "0.000000000001");
        assert_eq!(format_xmr(1_500_000_000_000), "1.5");
        assert_eq!(format_xmr(u64::MAX), "18446744.073709551615");
    }

    #[test]
    fn parses_exact_decimals() {
        assert_eq!(parse_decimal("0.00814200", 8), Some(814_200));
        assert_eq!(parse_decimal("1", 12), Some(PICONERO_PER_XMR));
        assert_eq!(parse_decimal("0.000000000001", 12), Some(1));
        assert_eq!(parse_decimal(".5", 1), Some(5));
        for bad in ["", ".", "-1", "1e3", "0.0000000000001", "1.2.3", "99999999999999999999"] {
            assert_eq!(parse_decimal(bad, 12), None, "{bad}");
        }
    }

    #[test]
    fn converts_sats_to_piconero_rounding_up() {
        // 1 XMR = 0.008142 BTC = 814,200 sats.
        assert_eq!(piconero_for_sats(814_200, 814_200), Some(PICONERO_PER_XMR));
        // 18,000 sats / 814,200 sats per XMR = 0.022107590272... XMR.
        assert_eq!(piconero_for_sats(18_000, 814_200), Some(22_107_590_273));
        assert_eq!(piconero_for_sats(1, 814_200), Some(1_228_200));
        assert_eq!(piconero_for_sats(1, 0), None);
        assert_eq!(piconero_for_sats(u64::MAX, 1), None);
    }
}
