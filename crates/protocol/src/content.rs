//! Vendor self-labels for listings and stores.
//!
//! A label is part of the signed body, so a relay cannot add, strip, or
//! change one without breaking the signature. Nothing here hides a record.
//! Each buyer's client decides what to do with the label.

use crate::Error;

/// How a vendor rates a listing or a whole store.
///
/// Ordered from least to most restricted, so `max` combines a store rating
/// with a listing rating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContentRating {
    General,
    Sensitive,
    Adult,
}

impl ContentRating {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentRating::General => "general",
            ContentRating::Sensitive => "sensitive",
            ContentRating::Adult => "adult",
        }
    }

    pub(crate) fn tag(self) -> u8 {
        match self {
            ContentRating::General => 0,
            ContentRating::Sensitive => 1,
            ContentRating::Adult => 2,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Result<Self, Error> {
        match tag {
            0 => Ok(ContentRating::General),
            1 => Ok(ContentRating::Sensitive),
            2 => Ok(ContentRating::Adult),
            _ => Err(Error::UnknownRating),
        }
    }
}

/// What a listing contains, as a bit set.
///
/// Bits this version does not name are kept as they were signed, so a
/// listing from a newer vendor still verifies here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentTags(u16);

impl ContentTags {
    pub const NUDITY: Self = Self(1 << 0);
    pub const VIOLENCE: Self = Self(1 << 1);
    pub const DRUGS: Self = Self(1 << 2);
    pub const WEAPONS: Self = Self(1 << 3);
    pub const GAMBLING: Self = Self(1 << 4);

    const NAMED: [(ContentTags, &'static str); 5] = [
        (Self::NUDITY, "nudity"),
        (Self::VIOLENCE, "violence"),
        (Self::DRUGS, "drugs"),
        (Self::WEAPONS, "weapons"),
        (Self::GAMBLING, "gambling"),
    ];

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Names of the tags this version knows, in bit order.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMED
            .iter()
            .filter(|(tag, _)| self.contains(*tag))
            .map(|(_, name)| *name)
            .collect()
    }
}

/// A listing's self-label: one rating plus any number of tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentLabel {
    pub rating: ContentRating,
    pub tags: ContentTags,
}

impl ContentLabel {
    pub const GENERAL: Self = Self {
        rating: ContentRating::General,
        tags: ContentTags::empty(),
    };

    pub const fn new(rating: ContentRating, tags: ContentTags) -> Self {
        Self { rating, tags }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratings_round_trip_and_order_by_restriction() {
        for rating in [
            ContentRating::General,
            ContentRating::Sensitive,
            ContentRating::Adult,
        ] {
            assert_eq!(ContentRating::from_tag(rating.tag()), Ok(rating));
        }
        assert_eq!(ContentRating::from_tag(3), Err(Error::UnknownRating));
        assert!(ContentRating::General < ContentRating::Sensitive);
        assert!(ContentRating::Sensitive < ContentRating::Adult);
    }

    #[test]
    fn tags_name_known_bits_and_keep_unknown_ones() {
        let tags = ContentTags::WEAPONS.with(ContentTags::NUDITY);
        assert_eq!(tags.names(), vec!["nudity", "weapons"]);
        let future = ContentTags::from_bits(ContentTags::DRUGS.bits() | 1 << 15);
        assert_eq!(future.names(), vec!["drugs"]);
        assert_eq!(future.bits() & 1 << 15, 1 << 15);
        assert!(ContentTags::empty().names().is_empty());
    }
}
