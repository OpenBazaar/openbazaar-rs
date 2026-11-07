// The price of one seller's package, from signed records only: the same
// rules as crates/protocol/src/pricing.rs, checked against its golden
// vectors (crates/protocol/vectors/pricing.json). The seller's node prices
// the order again on receipt; this is what the buyer sees before ordering
// and how they check the seller's quote.

import type { Coupon, ListingTerms, Offer } from "./commerce.ts";

export type Delivery = { service: number } | "pickup" | "digital";

export type QuoteLine = { listing: ListingTerms; variant: string; quantity: number; delivery: Delivery; offer?: Offer | null };

export type Quote = { currency: string; units: number[]; subtotals: number[]; offered: boolean[]; items: number; couponOff: number; shipping: number; total: number };

export type QuoteError =
  | "Empty" | "NotFiat" | "MixedStores" | "MixedCurrencies" | "NoSuchVariant" | "Away" | "BadQuantity" | "OverLimit"
  | "NoSuchService" | "NoPickup" | "WrongDelivery" | "NotShippable" | "BadOffer" | "OfferExpired" | "BadCoupon"
  | "CouponDates" | "CouponScope" | "CouponMinimum" | "Overflow";

export const QUOTE_ERRORS: Record<QuoteError, string> = {
  Empty: "The package is empty.",
  NotFiat: "A listing has no fiat price.",
  MixedStores: "A package is from one store.",
  MixedCurrencies: "A package is priced in one currency.",
  NoSuchVariant: "Choose an option the listing offers.",
  Away: "The seller is not taking orders for this listing.",
  BadQuantity: "A quantity is at least 1.",
  OverLimit: "That is more than the seller sells in one order.",
  NoSuchService: "Choose a shipping service the listing offers.",
  NoPickup: "The listing has no local pickup.",
  WrongDelivery: "Digital listings are delivered, physical ones shipped.",
  NotShippable: "The shipping service does not reach that country.",
  BadOffer: "The offer is not for this line or not from this seller.",
  OfferExpired: "The offer has expired.",
  BadCoupon: "The coupon is not from this seller.",
  CouponDates: "The coupon is not valid today.",
  CouponScope: "The coupon does not cover anything in this package.",
  CouponMinimum: "The package does not reach the coupon's minimum.",
  Overflow: "The amount is too large.",
};

export const NO_VARIANT = "0000000000000000";

export type At = { today: number; now: number; buyerKey?: string; country?: string };

class Refused extends Error {
  readonly code: QuoteError;
  constructor(code: QuoteError) { super(code); this.code = code; }
}

const refuse = (code: QuoteError): never => { throw new Refused(code); };

/// `bps` basis points of `amount`, rounded down.
export function off(amount: number, bps: number): number {
  return Number((BigInt(amount) * BigInt(bps)) / 10_000n);
}

function safe(value: number): number {
  return Number.isSafeInteger(value) ? value : refuse("Overflow");
}

function fiat(listing: ListingTerms) {
  return listing.price ?? refuse("NotFiat");
}

/// The unit price before an offer, the sale and tiers: the variant's own
/// price, or the listing's.
export function basePrice(listing: ListingTerms, variant: string): number {
  const listed = fiat(listing).amount;
  const commerce = listing.commerce;
  if (commerce && commerce.axes.length > 0) {
    const found = commerce.variants.find(v => v.id === variant) ?? refuse("NoSuchVariant");
    return found.price || listed;
  }
  if (variant !== NO_VARIANT) refuse("NoSuchVariant");
  return listed;
}

/// The unit price today, after any sale.
export function unitToday(listing: ListingTerms, variant: string, today: number): number {
  const unit = basePrice(listing, variant);
  const sale = listing.commerce?.sale;
  return sale && sale.startsOn <= today && today <= sale.endsOn ? unit - off(unit, sale.offBps) : unit;
}

function priceLine(line: QuoteLine, at: At) {
  if (line.quantity < 1) refuse("BadQuantity");
  const commerce = line.listing.commerce;
  const listedUnit = basePrice(line.listing, line.variant);
  if (commerce) {
    if (commerce.availability.kind === "away") refuse("Away");
    if (commerce.maxPerOrder && line.quantity > commerce.maxPerOrder) refuse("OverLimit");
  }
  if (line.offer) {
    const o = line.offer;
    const ok = o.storeId === line.listing.storeId && o.listingId === line.listing.listingId && o.variantId === line.variant
      && o.quantity === line.quantity && o.currency === fiat(line.listing).currency && (!at.buyerKey || at.buyerKey === o.buyerKey);
    if (!ok) refuse("BadOffer");
    if (at.now >= o.expiresAt) refuse("OfferExpired");
    return { unit: o.amount, subtotal: safe(o.amount * line.quantity), offered: true };
  }
  if (!commerce) return { unit: listedUnit, subtotal: safe(listedUnit * line.quantity), offered: false };
  const unit = unitToday(line.listing, line.variant, at.today);
  const gross = safe(unit * line.quantity);
  const tier = [...commerce.tiers].reverse().find(t => t.minQty <= line.quantity);
  return { unit, subtotal: gross - (tier ? off(gross, tier.offBps) : 0), offered: false };
}

type Shipped = { price: number; extraItem: number; quantity: number; freeOver: number };

function delivery(line: QuoteLine, at: At): Shipped | null {
  const digital = line.listing.kind === "digital";
  const shipping = line.listing.commerce?.shipping;
  if (line.delivery === "digital") return digital ? null : refuse("WrongDelivery");
  if (digital) refuse("WrongDelivery");
  if (line.delivery === "pickup") return shipping?.pickup ? null : refuse("NoPickup");
  const index = line.delivery.service;
  if (shipping && shipping.services.length > 0) {
    const service = shipping.services[index] ?? refuse("NoSuchService");
    if (at.country && service.regions.length > 0 && !service.regions.includes(at.country)) refuse("NotShippable");
    return { price: service.price, extraItem: service.extraItem, quantity: line.quantity, freeOver: shipping.freeOver };
  }
  if (index !== 0) refuse("NoSuchService");
  return { price: fiat(line.listing).shipping, extraItem: 0, quantity: line.quantity, freeOver: shipping?.freeOver ?? 0 };
}

function shippingFor(shipped: Shipped[], discounted: number): number {
  if (!shipped.length) return 0;
  let baseIndex = 0;
  shipped.forEach((s, i) => { if (s.price > shipped[baseIndex].price) baseIndex = i; });
  const base = shipped[baseIndex];
  if (base.freeOver && discounted >= base.freeOver) return 0;
  return safe(shipped.reduce((total, s, i) => total + s.extraItem * (i === baseIndex ? s.quantity - 1 : s.quantity), base.price));
}

function couponDiscount(lines: QuoteLine[], priced: { subtotal: number; offered: boolean }[], coupon: Coupon, currency: string, today: number): number {
  if (coupon.storeId !== lines[0].listing.storeId || coupon.currency !== currency) refuse("BadCoupon");
  if (today < coupon.startsOn || today > coupon.endsOn) refuse("CouponDates");
  const covered = lines
    .map((line, i) => ({ line, q: priced[i] }))
    .filter(({ line, q }) => !q.offered && line.listing.listingId !== null && (coupon.listings.length === 0 || coupon.listings.includes(line.listing.listingId)));
  if (!covered.length) refuse("CouponScope");
  const scoped = safe(covered.reduce((n, { q }) => n + q.subtotal, 0));
  if (scoped < coupon.minSubtotal) refuse("CouponMinimum");
  return coupon.discount.kind === "percent" ? off(scoped, coupon.discount.bps) : Math.min(coupon.discount.amount, scoped);
}

/// Price one package. `coupon` and offers must already be signature-checked
/// (`verifyCoupon`, `verifyOffer`).
export function quote(lines: QuoteLine[], coupon: Coupon | null, at: At): { ok: true; quote: Quote } | { ok: false; error: QuoteError; message: string } {
  try {
    if (!lines.length) refuse("Empty");
    const currency = fiat(lines[0].listing).currency;
    const priced = [];
    const shipped: Shipped[] = [];
    for (const line of lines) {
      if (line.listing.storeId !== lines[0].listing.storeId) refuse("MixedStores");
      if (fiat(line.listing).currency !== currency) refuse("MixedCurrencies");
      priced.push(priceLine(line, at));
      const ship = delivery(line, at);
      if (ship) shipped.push(ship);
    }
    const items = safe(priced.reduce((n, q) => n + q.subtotal, 0));
    const couponOff = coupon ? couponDiscount(lines, priced, coupon, currency, at.today) : 0;
    const shipping = shippingFor(shipped, items - couponOff);
    const total = safe(items - couponOff + shipping);
    return { ok: true, quote: { currency, units: priced.map(q => q.unit), subtotals: priced.map(q => q.subtotal), offered: priced.map(q => q.offered), items, couponOff, shipping, total } };
  } catch (error) {
    if (error instanceof Refused) return { ok: false, error: error.code, message: QUOTE_ERRORS[error.code] };
    throw error;
  }
}

/// Minor units per major unit, as in the protocol's FiatPrice::scale.
export function scaleOf(currency: string): number {
  return currency === "JPY" ? 1 : 100;
}

export function formatMinor(amount: number, currency: string): string {
  const digits = scaleOf(currency) === 1 ? 0 : 2;
  return new Intl.NumberFormat("en-US", { style: "currency", currency, minimumFractionDigits: digits, maximumFractionDigits: digits }).format(amount / scaleOf(currency));
}

/// "12.50" to 1250 cents; null when it is not a plain amount.
export function toMinor(text: string, currency: string): number | null {
  const trimmed = text.trim();
  const digits = scaleOf(currency) === 1 ? 0 : 2;
  if (!new RegExp(digits ? "^\\d+(\\.\\d{1,2})?$" : "^\\d+$").test(trimmed)) return null;
  const [whole, fraction = ""] = trimmed.split(".");
  return Number(whole) * scaleOf(currency) + Number(fraction.padEnd(digits, "0") || 0);
}

export function fromMinor(amount: number, currency: string): string {
  const scale = scaleOf(currency);
  return scale === 1 ? String(amount) : `${Math.floor(amount / scale)}.${String(amount % scale).padStart(2, "0")}`;
}
