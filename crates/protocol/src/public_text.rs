//! Refuse public free text that would contact or locate a person.
//!
//! Store and listing records are replicated to every node and never expire,
//! so their free-text fields must not carry an email address, a phone number,
//! a street address, or map coordinates. [`check`] runs when a record is
//! signed, not when one is decoded, so records signed before it existed
//! still verify.
//!
//! The checks are heuristics that err toward refusing. They catch the
//! ordinary ways of writing these things, not deliberate obfuscation:
//!
//! * **email**: a token with letters or digits on both sides of `@` and a
//!   dot after it, or the spelled-out `[at]` / `(at)` forms;
//! * **phone**: a run of at least 10 digits joined only by spaces, `+`, `-`,
//!   `.`, `(` or `)` (an ISBN-13 is refused too; put it in the title without
//!   separators broken by words, or leave it out);
//! * **street**: a house number of at most six digits (optionally one letter)
//!   followed within four words by a street-type word such as `Street`,
//!   `Ave`, or `Road`, or a `PO Box` with a number;
//! * **coordinates**: two decimal numbers with at least four fraction digits
//!   next to each other, or degrees followed by minutes (`45°31'`). A
//!   temperature such as `350°F` passes.
//! * **postal code**: a US ZIP after a two-letter state code (`OR 97214`), a
//!   UK postcode (`SW1A 1AA`), or a Canadian one (`K1A 0B1`).
//!
//! [`check_region`] also holds the store region and a listing's ship-from to
//! a state or country: no comma (`Portland, Oregon`), no digits, and at most
//! three words. A city name on its own cannot be told from a region and passes.

use crate::Error;

const STREET_TYPES: &[&str] = &[
    "street", "st", "avenue", "ave", "road", "rd", "boulevard", "blvd", "lane", "ln", "drive", "dr",
    "court", "ct", "place", "pl", "terrace", "ter", "highway", "hwy", "parkway", "pkwy", "circle",
    "cir", "way", "square", "sq", "alley", "trail", "crescent", "close",
];

/// What a refused field looked like it contained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateText {
    Email,
    Phone,
    StreetAddress,
    Coordinates,
    PostalCode,
    /// A region finer than a state or country.
    FineRegion,
}

impl PrivateText {
    pub fn as_str(self) -> &'static str {
        match self {
            PrivateText::Email => "an email address",
            PrivateText::Phone => "a phone number",
            PrivateText::StreetAddress => "a street address",
            PrivateText::Coordinates => "map coordinates",
            PrivateText::PostalCode => "a postal code",
            PrivateText::FineRegion => "a place finer than a state or country",
        }
    }
}

/// [`check`], and refuse a region that names more than a state or country.
pub fn check_region(field: &'static str, text: &str) -> Result<(), Error> {
    check(field, text)?;
    let fine = text.contains(',') || text.chars().any(|ch| ch.is_ascii_digit()) || text.split_whitespace().count() > 3;
    if fine {
        return Err(Error::PrivateText { field, kind: PrivateText::FineRegion });
    }
    Ok(())
}

/// Refuse `text` for the public field `field` if it looks like contact or location data.
pub fn check(field: &'static str, text: &str) -> Result<(), Error> {
    match find(text) {
        Some(kind) => Err(Error::PrivateText { field, kind }),
        None => Ok(()),
    }
}

/// The first kind of private text found in `text`, if any.
pub fn find(text: &str) -> Option<PrivateText> {
    if has_email(text) {
        Some(PrivateText::Email)
    } else if has_phone(text) {
        Some(PrivateText::Phone)
    } else if has_street(text) {
        Some(PrivateText::StreetAddress)
    } else if has_coordinates(text) {
        Some(PrivateText::Coordinates)
    } else if has_postal_code(text) {
        Some(PrivateText::PostalCode)
    } else {
        None
    }
}

/// `pattern` over `token`: `A` an uppercase ASCII letter, `9` a digit, `*` either.
fn shaped(token: &str, pattern: &str) -> bool {
    token.len() == pattern.len()
        && token.bytes().zip(pattern.bytes()).all(|(ch, want)| match want {
            b'A' => ch.is_ascii_uppercase(),
            b'9' => ch.is_ascii_digit(),
            _ => ch.is_ascii_uppercase() || ch.is_ascii_digit(),
        })
}

fn has_postal_code(text: &str) -> bool {
    let tokens: Vec<&str> = text
        .split_whitespace()
        .map(|token| token.trim_matches(|ch: char| ",.;:()".contains(ch)))
        .collect();
    tokens.windows(2).any(|pair| {
        let (first, second) = (pair[0], pair[1]);
        let zip = second.split_once('-').map_or(second, |(five, four)| if shaped(four, "9999") { five } else { "" });
        let us = shaped(first, "AA") && shaped(zip, "99999");
        let uk = ["A9", "A99", "AA9", "AA99", "A9A", "AA9A"].iter().any(|pattern| shaped(first, pattern)) && shaped(second, "9AA");
        let ca = shaped(first, "A9A") && shaped(second, "9A9");
        us || uk || ca
    })
}

fn has_email(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("[at]") || lower.contains("(at)") {
        return true;
    }
    lower.split(|ch: char| ch.is_whitespace() || "<>,;:\"'()[]".contains(ch)).any(|token| {
        let Some((local, domain)) = token.split_once('@') else {
            return false;
        };
        let domain = domain.trim_end_matches(['.', '!', '?']);
        local.chars().any(|ch| ch.is_alphanumeric())
            && domain
                .split_once('.')
                .is_some_and(|(host, tld)| !host.is_empty() && tld.chars().any(|ch| ch.is_alphabetic()))
    })
}

fn has_phone(text: &str) -> bool {
    let mut digits = 0;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
            if digits >= 10 {
                return true;
            }
        } else if !" +-.()".contains(ch) {
            digits = 0;
        }
    }
    false
}

fn word(token: &str) -> String {
    token
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn is_house_number(token: &str) -> bool {
    let token = token.trim_end_matches([',', '.']);
    let digits = token.chars().take_while(|ch| ch.is_ascii_digit()).count();
    let rest = &token[digits..];
    (1..=6).contains(&digits) && (rest.is_empty() || (rest.len() == 1 && rest.chars().all(|ch| ch.is_ascii_alphabetic())))
}

fn has_street(text: &str) -> bool {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        let lower = word(token);
        if (lower == "box" || lower == "pobox") && i > 0 {
            let before = word(tokens[i - 1]);
            if (before == "po" || before == "p.o" || before == "p.o." || before == "post" || lower == "pobox")
                && tokens.get(i + 1).is_some_and(|next| is_house_number(next))
            {
                return true;
            }
        }
        if !is_house_number(token) {
            continue;
        }
        for (offset, next) in tokens.iter().enumerate().skip(i + 1).take(4) {
            let name = word(next);
            if name.is_empty() || !name.chars().all(|ch| ch.is_alphabetic()) {
                break;
            }
            if offset > i + 1 && STREET_TYPES.contains(&name.as_str()) {
                return true;
            }
        }
    }
    false
}

fn has_coordinates(text: &str) -> bool {
    let mut previous_precise = false;
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if !ch.is_ascii_digit() {
            if ch.is_alphabetic() {
                previous_precise = false;
            }
            continue;
        }
        let mut end = start + 1;
        while let Some(&(index, next)) = chars.peek() {
            if next.is_ascii_digit() || next == '.' {
                end = index + next.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let number = &text[start..end];
        if let Some(after) = text[end..].strip_prefix('°') {
            let after = after.trim_start();
            let minutes = after.chars().take_while(|ch| ch.is_ascii_digit()).count();
            if minutes > 0 && after[minutes..].starts_with(['\'', '′']) {
                return true;
            }
        }
        let precise = number
            .split_once('.')
            .is_some_and(|(_, fraction)| fraction.len() >= 4 && fraction.chars().all(|ch| ch.is_ascii_digit()));
        if precise && previous_precise {
            return true;
        }
        previous_precise = precise;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_and_location_text_is_refused() {
        for (text, kind) in [
            ("Write to ada@example.com for custom sizes.", PrivateText::Email),
            ("ada [at] example dot com", PrivateText::Email),
            ("Call (503) 555-0142 before noon.", PrivateText::Phone),
            ("+44 20 7946 0958", PrivateText::Phone),
            ("Pick up at 12 Cedar Street.", PrivateText::StreetAddress),
            ("Studio: 4417B North Mississippi Ave, Portland", PrivateText::StreetAddress),
            ("Mail to PO Box 118", PrivateText::StreetAddress),
            ("Find us at 45.5231, -122.6765", PrivateText::Coordinates),
            ("45°31'N 122°40'W", PrivateText::Coordinates),
            ("Portland OR 97214", PrivateText::PostalCode),
            ("Ships from VT 05401-1234.", PrivateText::PostalCode),
            ("London SW1A 1AA", PrivateText::PostalCode),
            ("Ottawa, ON K1A 0B1", PrivateText::PostalCode),
        ] {
            assert_eq!(find(text), Some(kind), "{text}");
            assert_eq!(check("about", text), Err(Error::PrivateText { field: "about", kind }));
        }
    }

    #[test]
    fn ordinary_shop_text_passes() {
        for text in [
            "A 12 ounce stoneware mug with a dull white glaze.",
            "Undyed linen, hemmed, about 20 by 28 inches.",
            "Five hundred sheets of cotton paper, 90 gsm.",
            "A 150 centimeter linen tape with metal tips.",
            "Ships within 3 business days. Returns accepted for 30 days.",
            "Price 18,000 sats; 2 for 35,000.",
            "Version 1.2.3 of the pattern, rev 2026.",
            "Twelve short etudes for late-intermediate piano.",
            "Grade A maple syrup in an 8 ounce glass bottle.",
            "Bake at 350°F for 40 minutes.",
            "A 3 way switch.",
            "Oregon",
            "Ships in 2 to 4 days; free over 20,000 sats.",
            "A size M2 bolt and a 10 mm nut.",
            "",
        ] {
            assert_eq!(find(text), None, "{text}");
        }
    }

    #[test]
    fn a_region_is_a_state_or_country() {
        for region in ["Oregon", "Vermont", "New South Wales", "Digital", ""] {
            assert_eq!(check_region("store region", region), Ok(()), "{region}");
        }
        for region in ["Portland, Oregon", "Zone 3", "North end of Portland Oregon", "OR 97214"] {
            assert!(matches!(check_region("store region", region), Err(Error::PrivateText { .. })), "{region}");
        }
    }
}
