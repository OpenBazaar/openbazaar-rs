//! What a version 7 listing adds to version 6: condition, item specifics,
//! variants, pricing rules, purchase terms, shipping and a returns override.
//!
//! Every amount is in the listing's fiat currency (its v6 [`FiatPrice`]), in
//! minor units. Days are calendar days as YYYYMMDD. Anything a buyer relies
//! on is here, inside the store's signature; the seller's private rules
//! (an offer floor, live stock) stay on the seller's node.
//!
//! [`FiatPrice`]: crate::FiatPrice

use crate::encode::{Reader, Writer};
use crate::public_text;
use crate::shipping::Shipping;
use crate::variants::Variants;
use crate::Error;

pub const MAX_SPECIFICS: usize = 24;
pub const MAX_TIERS: usize = 4;
const MAX_CONDITION_NOTE: usize = 280;
const MAX_SPECIFIC_KEY: usize = 32;
const MAX_SPECIFIC_VALUE: usize = 120;
const MAX_BPS: u16 = 9_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Condition {
    #[default]
    New,
    OpenBox,
    LikeNew,
    Good,
    Fair,
    /// For parts or not working. Mercari calls this "Poor".
    ForParts,
}

impl Condition {
    pub const ALL: [Condition; 6] = [
        Condition::New,
        Condition::OpenBox,
        Condition::LikeNew,
        Condition::Good,
        Condition::Fair,
        Condition::ForParts,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Condition::New => "new",
            Condition::OpenBox => "open-box",
            Condition::LikeNew => "like-new",
            Condition::Good => "good",
            Condition::Fair => "fair",
            Condition::ForParts => "for-parts",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|condition| condition.as_str() == text)
    }

    fn tag(self) -> u8 {
        Self::ALL.iter().position(|&c| c == self).expect("listed") as u8
    }

    fn from_tag(tag: u8) -> Result<Self, Error> {
        Self::ALL.get(tag as usize).copied().ok_or(Error::BadListing("unknown condition"))
    }
}

/// A timed sale: a percentage off every variant between two days, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sale {
    pub off_bps: u16,
    pub starts_on: u32,
    pub ends_on: u32,
}

impl Sale {
    pub fn active_on(&self, day: u32) -> bool {
        (self.starts_on..=self.ends_on).contains(&day)
    }
}

/// Buy at least `min_qty` of one line, get `off_bps` off that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier {
    pub min_qty: u16,
    pub off_bps: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pricing {
    /// A reference ("list") price shown struck through. Zero means none.
    pub compare_at: u64,
    pub sale: Option<Sale>,
    /// Strictly increasing in both quantity and discount.
    pub tiers: Vec<Tier>,
    pub accepts_offers: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Availability {
    #[default]
    InStock,
    /// Taking orders now; ships on this day.
    PreOrder { ships_on: u32 },
    /// Made after the order; ships this many days later. Stock zero means unlimited.
    MadeToOrder { days: u16 },
    /// Paused: shown, not sold.
    Away,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Purchase {
    /// Zero means no limit.
    pub max_per_order: u16,
    /// Pieces in one unit ("lot of 5"). At least 1.
    pub lot_size: u16,
    pub availability: Availability,
}

impl Default for Purchase {
    fn default() -> Self {
        Self { max_per_order: 0, lot_size: 1, availability: Availability::InStock }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Returns {
    /// The store record's returns policy applies.
    #[default]
    Store,
    NoReturns,
    Days { days: u16, seller_pays: bool },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commerce {
    pub condition: Condition,
    pub condition_note: String,
    /// Top-level category, see [`crate::taxonomy`]. Zero means uncategorised.
    pub taxonomy: u16,
    /// Item specifics such as brand or size, sorted by key, keys unique.
    pub specifics: Vec<(String, String)>,
    pub variants: Variants,
    pub pricing: Pricing,
    pub purchase: Purchase,
    pub shipping: Shipping,
    pub returns: Returns,
}

/// What [`Commerce::check`] needs from the rest of the listing.
pub(crate) struct Context {
    pub price: u64,
    pub digital: bool,
    pub images: usize,
}

impl Commerce {
    pub fn specific(&self, key: &str) -> Option<&str> {
        self.specifics.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub(crate) fn check(&self, context: &Context) -> Result<(), Error> {
        if self.condition_note.len() > MAX_CONDITION_NOTE {
            return Err(Error::BadListing("a condition note is at most 280 bytes"));
        }
        self.check_specifics()?;
        self.variants.check(context.images)?;
        self.check_pricing(context.price)?;
        if self.purchase.lot_size == 0 {
            return Err(Error::BadListing("a unit is at least one piece"));
        }
        if let Returns::Days { days, .. } = self.returns {
            if days == 0 || days > 365 {
                return Err(Error::BadListing("a returns window is 1 to 365 days"));
            }
        }
        self.shipping.check(context.digital)
    }

    fn check_specifics(&self) -> Result<(), Error> {
        if self.specifics.len() > MAX_SPECIFICS {
            return Err(Error::BadListing("a listing has at most 24 item specifics"));
        }
        if !self.specifics.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return Err(Error::BadListing("item specifics are sorted by key, each key once"));
        }
        for (key, value) in &self.specifics {
            let canonical = !key.is_empty()
                && key.len() <= MAX_SPECIFIC_KEY
                && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !canonical {
                return Err(Error::BadListing("an item specific key is lowercase letters, digits and dashes"));
            }
            if value.trim().is_empty() || value.len() > MAX_SPECIFIC_VALUE {
                return Err(Error::BadListing("an item specific value is empty or too long"));
            }
            if is_code_key(key) && !valid_code(key, value) {
                return Err(Error::BadListing("a product code's check digit is wrong"));
            }
        }
        Ok(())
    }

    fn check_pricing(&self, price: u64) -> Result<(), Error> {
        let pricing = &self.pricing;
        let highest = self
            .variants
            .variants
            .iter()
            .map(|variant| if variant.price == 0 { price } else { variant.price })
            .max()
            .unwrap_or(price);
        if pricing.compare_at != 0 && pricing.compare_at <= highest {
            return Err(Error::BadListing("a compare-at price is above every price it is compared with"));
        }
        if let Some(sale) = pricing.sale {
            if sale.off_bps == 0 || sale.off_bps > MAX_BPS {
                return Err(Error::BadListing("a sale takes 0.01% to 90% off"));
            }
            if sale.starts_on > sale.ends_on {
                return Err(Error::BadListing("a sale ends before it starts"));
            }
        }
        if pricing.tiers.len() > MAX_TIERS {
            return Err(Error::BadListing("a listing has at most 4 volume tiers"));
        }
        for (i, tier) in pricing.tiers.iter().enumerate() {
            if tier.min_qty < 2 || tier.off_bps == 0 || tier.off_bps > MAX_BPS {
                return Err(Error::BadListing("a volume tier starts at 2 and takes 0.01% to 90% off"));
            }
            if i > 0 {
                let before = pricing.tiers[i - 1];
                if tier.min_qty <= before.min_qty || tier.off_bps <= before.off_bps {
                    return Err(Error::BadListing("volume tiers rise in quantity and discount"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn check_public_text(&self) -> Result<(), Error> {
        public_text::check("condition note", &self.condition_note)?;
        for (key, value) in &self.specifics {
            if !is_code_key(key) {
                public_text::check("item specific", value)?;
            }
        }
        self.variants.check_public_text()?;
        self.shipping.check_public_text()
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), Error> {
        writer.u8(self.condition.tag());
        writer.str(&self.condition_note)?;
        writer.u16(self.taxonomy);
        writer.u8(self.specifics.len() as u8);
        for (key, value) in &self.specifics {
            writer.str(key)?;
            writer.str(value)?;
        }
        self.variants.encode(writer)?;
        let pricing = &self.pricing;
        writer.u64(pricing.compare_at);
        match pricing.sale {
            None => writer.u8(0),
            Some(sale) => {
                writer.u8(1);
                writer.u16(sale.off_bps);
                writer.u32(sale.starts_on);
                writer.u32(sale.ends_on);
            }
        }
        writer.u8(pricing.tiers.len() as u8);
        for tier in &pricing.tiers {
            writer.u16(tier.min_qty);
            writer.u16(tier.off_bps);
        }
        writer.u8(u8::from(pricing.accepts_offers));
        writer.u16(self.purchase.max_per_order);
        writer.u16(self.purchase.lot_size);
        let (tag, value) = match self.purchase.availability {
            Availability::InStock => (0, 0),
            Availability::PreOrder { ships_on } => (1, ships_on),
            Availability::MadeToOrder { days } => (2, u32::from(days)),
            Availability::Away => (3, 0),
        };
        writer.u8(tag);
        writer.u32(value);
        self.shipping.encode(writer)?;
        let (tag, days) = match self.returns {
            Returns::Store => (0, 0),
            Returns::NoReturns => (1, 0),
            Returns::Days { days, seller_pays: false } => (2, days),
            Returns::Days { days, seller_pays: true } => (3, days),
        };
        writer.u8(tag);
        writer.u16(days);
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self, Error> {
        let condition = Condition::from_tag(reader.u8()?)?;
        let condition_note = reader.str()?;
        let taxonomy = reader.u16()?;
        let count = reader.u8()? as usize;
        if count > MAX_SPECIFICS {
            return Err(Error::BadListing("a listing has at most 24 item specifics"));
        }
        let specifics = (0..count).map(|_| Ok((reader.str()?, reader.str()?))).collect::<Result<_, Error>>()?;
        let variants = Variants::decode(reader)?;
        let compare_at = reader.u64()?;
        let sale = match reader.u8()? {
            0 => None,
            1 => Some(Sale { off_bps: reader.u16()?, starts_on: reader.u32()?, ends_on: reader.u32()? }),
            _ => return Err(Error::BadListing("unknown sale tag")),
        };
        let tier_count = reader.u8()? as usize;
        if tier_count > MAX_TIERS {
            return Err(Error::BadListing("a listing has at most 4 volume tiers"));
        }
        let tiers = (0..tier_count)
            .map(|_| Ok(Tier { min_qty: reader.u16()?, off_bps: reader.u16()? }))
            .collect::<Result<_, Error>>()?;
        let accepts_offers = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(Error::BadListing("unknown offers flag")),
        };
        let max_per_order = reader.u16()?;
        let lot_size = reader.u16()?;
        let availability = match (reader.u8()?, reader.u32()?) {
            (0, 0) => Availability::InStock,
            (1, ships_on) => Availability::PreOrder { ships_on },
            (2, days) => Availability::MadeToOrder {
                days: u16::try_from(days).map_err(|_| Error::BadListing("made-to-order days out of range"))?,
            },
            (3, 0) => Availability::Away,
            _ => return Err(Error::BadListing("unknown availability")),
        };
        let shipping = Shipping::decode(reader)?;
        let returns = match (reader.u8()?, reader.u16()?) {
            (0, 0) => Returns::Store,
            (1, 0) => Returns::NoReturns,
            (2, days) => Returns::Days { days, seller_pays: false },
            (3, days) => Returns::Days { days, seller_pays: true },
            _ => return Err(Error::BadListing("unknown returns policy")),
        };
        Ok(Self {
            condition,
            condition_note,
            taxonomy,
            specifics,
            variants,
            pricing: Pricing { compare_at, sale, tiers, accepts_offers },
            purchase: Purchase { max_per_order, lot_size, availability },
            shipping,
            returns,
        })
    }
}

/// Product-code specifics are digits with a check digit, not free text, so
/// the phone-number heuristic does not apply to them.
fn is_code_key(key: &str) -> bool {
    matches!(key, "gtin" | "upc" | "ean" | "isbn")
}

fn valid_code(key: &str, value: &str) -> bool {
    if key == "isbn" && value.len() == 10 {
        return isbn10(value);
    }
    let lengths: &[usize] = match key {
        "upc" => &[12],
        "ean" => &[8, 13],
        "isbn" => &[13],
        _ => &[8, 12, 13, 14],
    };
    lengths.contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit()) && gtin_check(value)
}

/// GS1 mod-10: weights 3 and 1 from the right, excluding the check digit.
fn gtin_check(digits: &str) -> bool {
    let values: Vec<u32> = digits.bytes().map(|b| u32::from(b - b'0')).collect();
    let (check, body) = values.split_last().expect("non-empty");
    let sum: u32 = body.iter().rev().enumerate().map(|(i, d)| if i % 2 == 0 { d * 3 } else { *d }).sum();
    (10 - sum % 10) % 10 == *check
}

fn isbn10(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut sum = 0u32;
    for (i, &b) in bytes.iter().enumerate() {
        let digit = match b {
            b'0'..=b'9' => u32::from(b - b'0'),
            b'X' if i == 9 => 10,
            _ => return false,
        };
        sum += digit * (10 - i as u32);
    }
    sum % 11 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_codes_need_a_valid_check_digit() {
        assert!(valid_code("upc", "036000291452"));
        assert!(!valid_code("upc", "036000291453"));
        assert!(valid_code("isbn", "9780306406157"));
        assert!(valid_code("isbn", "080442957X"));
        assert!(!valid_code("isbn", "0804429570"));
        assert!(valid_code("ean", "96385074"));
        assert!(!valid_code("gtin", "12345"));
    }
}
