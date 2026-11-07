//! RFC 4648 base32 without padding, as the protocol writes discovery secrets.

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut value, mut bits) = (0u32, 0u32);
    for ch in text.trim().trim_end_matches('=').bytes() {
        let digit = ALPHABET
            .iter()
            .position(|&a| a == ch.to_ascii_lowercase())? as u32;
        value = (value << 5 | digit) & 0x1fff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((value >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut value, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        value = (value << 8 | byte as u32) & 0x1fff;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((value >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((value << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_rfc4648_vectors() {
        assert_eq!(encode(b"foobar"), "mzxw6ytboi");
        assert_eq!(decode("MZXW6YTBOI======").unwrap(), b"foobar");
        assert_eq!(decode(&encode(&[0xa5; 32])).unwrap(), vec![0xa5; 32]);
        assert!(decode("not base32!").is_none());
    }
}
