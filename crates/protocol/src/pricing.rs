//! The price of one seller's package, from signed records only.
//!
//! The buyer's client and the seller's node run the same rules, so the quote
//! a seller returns can be checked. Per line: an accepted offer's amount, else
//! the variant's (or listing's) price less any sale running today, then the
//! highest volume tier the quantity reaches. Then a coupon on the lines it
//! covers (offer lines excluded). Shipping: the line with the dearest service
//! pays that service's first-item price, every other unit its service's
//! extra-item price; free when the discounted subtotal reaches the free
//! shipping threshold of that dearest line. Discounts round down, so the
//! seller never charges less than what each rule says.
//!
//! Live stock is not checked here: the seller's node checks it on receipt.

use crate::coupon::{Discount, SignedCoupon};
use crate::listing::{Listing, ListingKind};
use crate::offer::SignedOffer;
use crate::sign::PUBKEY_LEN;
use crate::store::CountryCode;
use crate::variants::VariantId;
use crate::commerce::Availability;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Index into the listing's shipping services. A physical listing
    /// without version 7 services has one implicit service, index 0, at its
    /// version 6 flat shipping price.
    Service(u8),
    Pickup,
    Digital,
}

#[derive(Clone, Debug)]
pub struct Line<'a> {
    pub listing: &'a Listing,
    pub variant: VariantId,
    pub quantity: u32,
    pub delivery: Delivery,
    pub offer: Option<&'a SignedOffer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineQuote {
    pub unit: u64,
    /// `unit * quantity` less the volume tier.
    pub subtotal: u64,
    pub offered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    pub currency: String,
    pub lines: Vec<LineQuote>,
    pub items: u64,
    pub coupon_off: u64,
    pub shipping: u64,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteError {
    Empty,
    NotFiat,
    MixedStores,
    MixedCurrencies,
    NoSuchVariant,
    Away,
    BadQuantity,
    OverLimit,
    NoSuchService,
    NoPickup,
    WrongDelivery,
    NotShippable,
    BadOffer,
    OfferExpired,
    BadCoupon,
    CouponDates,
    CouponScope,
    CouponMinimum,
    Overflow,
}

impl QuoteError {
    pub fn as_str(self) -> &'static str {
        match self {
            QuoteError::Empty => "the package is empty",
            QuoteError::NotFiat => "a listing has no fiat price",
            QuoteError::MixedStores => "a package is from one store",
            QuoteError::MixedCurrencies => "a package is priced in one currency",
            QuoteError::NoSuchVariant => "choose an option the listing offers",
            QuoteError::Away => "the seller is not taking orders for this listing",
            QuoteError::BadQuantity => "a quantity is at least 1",
            QuoteError::OverLimit => "that is more than the seller sells in one order",
            QuoteError::NoSuchService => "choose a shipping service the listing offers",
            QuoteError::NoPickup => "the listing has no local pickup",
            QuoteError::WrongDelivery => "digital listings are delivered, physical ones shipped",
            QuoteError::NotShippable => "the shipping service does not reach that country",
            QuoteError::BadOffer => "the offer is not for this line or not from this seller",
            QuoteError::OfferExpired => "the offer has expired",
            QuoteError::BadCoupon => "the coupon is not from this seller",
            QuoteError::CouponDates => "the coupon is not valid today",
            QuoteError::CouponScope => "the coupon does not cover anything in this package",
            QuoteError::CouponMinimum => "the package does not reach the coupon's minimum",
            QuoteError::Overflow => "the amount is too large",
        }
    }
}

/// When and for whom the quote is made.
#[derive(Clone, Copy, Debug)]
pub struct At<'a> {
    /// YYYYMMDD.
    pub today: u32,
    /// Unix seconds.
    pub now: u64,
    /// The order's conversation key, which an offer must be bound to.
    pub buyer_key: Option<&'a [u8; PUBKEY_LEN]>,
    /// Where the package goes, when the caller knows.
    pub country: Option<&'a CountryCode>,
}

struct Shipped {
    price: u64,
    extra_item: u64,
    quantity: u64,
    free_over: u64,
}

pub fn quote(lines: &[Line<'_>], coupon: Option<&SignedCoupon>, at: At<'_>) -> Result<Quote, QuoteError> {
    let first = lines.first().ok_or(QuoteError::Empty)?;
    let currency = fiat(first.listing)?.currency.clone();
    let mut quotes = Vec::with_capacity(lines.len());
    let mut shipped = Vec::new();
    for line in lines {
        if line.listing.store_id != first.listing.store_id {
            return Err(QuoteError::MixedStores);
        }
        if fiat(line.listing)?.currency != currency {
            return Err(QuoteError::MixedCurrencies);
        }
        quotes.push(price_line(line, at)?);
        if let Some(ship) = delivery(line, at)? {
            shipped.push(ship);
        }
    }
    let items = sum(quotes.iter().map(|q| q.subtotal))?;
    let coupon_off = match coupon {
        Some(coupon) => coupon_discount(lines, &quotes, coupon, &currency, at.today)?,
        None => 0,
    };
    let discounted = items - coupon_off;
    let shipping = shipping_for(&shipped, discounted)?;
    let total = discounted.checked_add(shipping).ok_or(QuoteError::Overflow)?;
    Ok(Quote { currency, lines: quotes, items, coupon_off, shipping, total })
}

fn fiat(listing: &Listing) -> Result<&crate::FiatPrice, QuoteError> {
    listing.details.as_ref().and_then(|details| details.price.as_ref()).ok_or(QuoteError::NotFiat)
}

fn price_line(line: &Line<'_>, at: At<'_>) -> Result<LineQuote, QuoteError> {
    if line.quantity == 0 {
        return Err(QuoteError::BadQuantity);
    }
    let listed = fiat(line.listing)?.amount;
    let commerce = line.listing.commerce.as_ref();
    let unit = match commerce {
        Some(commerce) if !commerce.variants.is_empty() => {
            let variant = commerce.variants.get(line.variant).ok_or(QuoteError::NoSuchVariant)?;
            if variant.price == 0 { listed } else { variant.price }
        }
        _ if line.variant != VariantId::NONE => return Err(QuoteError::NoSuchVariant),
        _ => listed,
    };
    if let Some(commerce) = commerce {
        if commerce.purchase.availability == Availability::Away {
            return Err(QuoteError::Away);
        }
        let max = u32::from(commerce.purchase.max_per_order);
        if max != 0 && line.quantity > max {
            return Err(QuoteError::OverLimit);
        }
    }
    let quantity = u64::from(line.quantity);
    if let Some(offer) = line.offer {
        let amount = offered(line, offer, at)?;
        let subtotal = amount.checked_mul(quantity).ok_or(QuoteError::Overflow)?;
        return Ok(LineQuote { unit: amount, subtotal, offered: true });
    }
    let Some(commerce) = commerce else {
        let subtotal = unit.checked_mul(quantity).ok_or(QuoteError::Overflow)?;
        return Ok(LineQuote { unit, subtotal, offered: false });
    };
    let pricing = &commerce.pricing;
    let unit = match pricing.sale {
        Some(sale) if sale.active_on(at.today) => unit - off(unit, sale.off_bps),
        _ => unit,
    };
    let gross = unit.checked_mul(quantity).ok_or(QuoteError::Overflow)?;
    let tier = pricing.tiers.iter().rev().find(|tier| u64::from(tier.min_qty) <= quantity);
    let subtotal = gross - tier.map_or(0, |tier| off(gross, tier.off_bps));
    Ok(LineQuote { unit, subtotal, offered: false })
}

fn offered(line: &Line<'_>, offer: &SignedOffer, at: At<'_>) -> Result<u64, QuoteError> {
    let terms = &offer.terms;
    let matches = offer.verify().is_ok()
        && terms.store_id == line.listing.store_id
        && Some(terms.listing_id) == line.listing.id()
        && terms.variant_id == line.variant
        && u32::from(terms.quantity) == line.quantity
        && Some(&terms.currency) == fiat(line.listing).ok().map(|price| &price.currency)
        && at.buyer_key.is_none_or(|key| *key == terms.buyer_key);
    if !matches {
        return Err(QuoteError::BadOffer);
    }
    if !terms.live_at(at.now) {
        return Err(QuoteError::OfferExpired);
    }
    Ok(terms.amount)
}

fn delivery(line: &Line<'_>, at: At<'_>) -> Result<Option<Shipped>, QuoteError> {
    let digital = line.listing.kind == ListingKind::Digital;
    let shipping = line.listing.commerce.as_ref().map(|commerce| &commerce.shipping);
    match (line.delivery, digital) {
        (Delivery::Digital, true) => Ok(None),
        (Delivery::Digital, false) | (_, true) => Err(QuoteError::WrongDelivery),
        (Delivery::Pickup, false) => match shipping.and_then(|s| s.pickup.as_ref()) {
            Some(_) => Ok(None),
            None => Err(QuoteError::NoPickup),
        },
        (Delivery::Service(index), false) => {
            let quantity = u64::from(line.quantity);
            match shipping {
                Some(shipping) if !shipping.services.is_empty() => {
                    let service = shipping.services.get(index as usize).ok_or(QuoteError::NoSuchService)?;
                    if at.country.is_some_and(|country| !service.reaches(country)) {
                        return Err(QuoteError::NotShippable);
                    }
                    Ok(Some(Shipped { price: service.price, extra_item: service.extra_item, quantity, free_over: shipping.free_over }))
                }
                _ if index == 0 => {
                    let free_over = shipping.map_or(0, |s| s.free_over);
                    Ok(Some(Shipped { price: fiat(line.listing)?.shipping, extra_item: 0, quantity, free_over }))
                }
                _ => Err(QuoteError::NoSuchService),
            }
        }
    }
}

fn shipping_for(shipped: &[Shipped], discounted: u64) -> Result<u64, QuoteError> {
    let Some(base) = shipped.iter().enumerate().max_by_key(|(i, ship)| (ship.price, std::cmp::Reverse(*i))) else {
        return Ok(0);
    };
    let (base_index, base) = base;
    if base.free_over != 0 && discounted >= base.free_over {
        return Ok(0);
    }
    let mut total = base.price;
    for (i, ship) in shipped.iter().enumerate() {
        let extra_units = if i == base_index { ship.quantity - 1 } else { ship.quantity };
        let extra = ship.extra_item.checked_mul(extra_units).ok_or(QuoteError::Overflow)?;
        total = total.checked_add(extra).ok_or(QuoteError::Overflow)?;
    }
    Ok(total)
}

fn coupon_discount(
    lines: &[Line<'_>],
    quotes: &[LineQuote],
    coupon: &SignedCoupon,
    currency: &str,
    today: u32,
) -> Result<u64, QuoteError> {
    let terms = &coupon.terms;
    if coupon.verify().is_err() || terms.store_id != lines[0].listing.store_id || terms.currency != currency {
        return Err(QuoteError::BadCoupon);
    }
    if !terms.valid_on(today) {
        return Err(QuoteError::CouponDates);
    }
    let covered = lines
        .iter()
        .zip(quotes)
        .filter(|(line, quote)| !quote.offered && line.listing.id().is_some_and(|id| terms.applies_to(&id)))
        .map(|(_, quote)| quote.subtotal);
    let scoped = sum(covered.clone())?;
    if covered.count() == 0 {
        return Err(QuoteError::CouponScope);
    }
    if scoped < terms.min_subtotal {
        return Err(QuoteError::CouponMinimum);
    }
    Ok(match terms.discount {
        Discount::Percent(bps) => off(scoped, bps),
        Discount::Fixed(amount) => amount.min(scoped),
    })
}

/// `bps` basis points of `amount`, rounded down.
fn off(amount: u64, bps: u16) -> u64 {
    (u128::from(amount) * u128::from(bps) / 10_000) as u64
}

fn sum(mut values: impl Iterator<Item = u64>) -> Result<u64, QuoteError> {
    values.try_fold(0u64, |acc, value| acc.checked_add(value)).ok_or(QuoteError::Overflow)
}
