use std::fmt;
use std::str::FromStr;

use safelog::DisplayRedacted;
use tor_hscrypto::pk::HsId;
pub use tor_hscrypto::pk::HsIdParseError;

/// Address from Arti's own `HsId` test vector. Not a live service.
#[cfg(test)]
pub(crate) const TEST_ONION: &str =
    "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion";

use crate::Error;

/// Virtual port the order letter is written to.
///
/// The listener accepts `BEGIN` for this port and rejects every other port.
pub const ORDER_PORT: u16 = 80;

/// A validated v3 onion service address.
///
/// Parsing goes through Arti's [`HsId`], which checks the `.onion` suffix,
/// the version byte, and the rend-spec checksum. This type does not speak
/// the Tor protocol itself.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct OnionAddress {
    id: HsId,
    text: String,
}

impl OnionAddress {
    /// Parse `input` as a v3 onion address.
    ///
    /// The checksum is Arti's. A wrong character, a v2 address, or a name
    /// that merely ends in `.onion` is rejected.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let id = HsId::from_str(input).map_err(Error::BadOnion)?;
        Ok(Self::from_hs_id(id))
    }

    pub(crate) fn from_hs_id(id: HsId) -> Self {
        let text = id.display_unredacted().to_string();
        Self { id, text }
    }

    /// Canonical lowercase `base32.onion` form.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The Arti identity this address names.
    pub fn hs_id(&self) -> &HsId {
        &self.id
    }
}

impl FromStr for OnionAddress {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for OnionAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl fmt::Debug for OnionAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("OnionAddress")
            .field(&self.text)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v3_and_roundtrips_canonical_text() {
        let address = OnionAddress::parse(TEST_ONION).unwrap();
        assert_eq!(address.as_str(), TEST_ONION);
        assert_eq!(address.to_string(), TEST_ONION);

        let mixed: String = TEST_ONION
            .chars()
            .enumerate()
            .map(|(index, ch)| {
                if index % 2 == 0 {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                }
            })
            .collect();
        assert_eq!(OnionAddress::parse(&mixed).unwrap(), address);
    }

    #[test]
    fn rejects_bad_onion_addresses() {
        let cases = [
            "not-an-onion",
            "example.com",
            "facebookcorewwwi.onion",
            "short.onion",
            "a.b.25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion",
            "",
            "25njqamcweflpvkl73j4szahhihoc4xt3ktcgjnpaingr5yhkenl5sid.onion.extra",
        ];
        for case in cases {
            assert!(OnionAddress::parse(case).is_err(), "{case}");
        }

        let mut broken = TEST_ONION.to_owned();
        broken.replace_range(0..1, "3");
        assert!(
            OnionAddress::parse(&broken).is_err(),
            "checksum must fail after a one-character edit"
        );
    }
}
