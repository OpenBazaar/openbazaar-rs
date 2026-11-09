import { useState } from "react";
import type { CartPackage } from "../cart";
import { termsOf } from "../cart";
import type { ListingHeader } from "../catalog";
import { basePrice, formatMinor, NO_VARIANT } from "../pricing";
import { initialSelection, listingSellable, maxQuantity, selectedVariant, sellable, type Selection } from "../variants";
import { ListingFacts, PriceTag, QuantityStepper, useListingStyles, VariantPicker } from "./VariantPicker";

export { QuantityStepper };

/** What the buyer picked on a version 7 listing. */
export type BuyChoice = { variant?: string; quantity: number };

function today(): number {
  const now = new Date();
  return now.getUTCFullYear() * 10_000 + (now.getUTCMonth() + 1) * 100 + now.getUTCDate();
}

/// The buyer's variant and quantity on a listing page, and whether that can be bought.
export function useBuyChoice(listing: ListingHeader) {
  const commerce = listing.commerce ?? null;
  const [selection, setSelection] = useState<Selection>(() => (commerce ? initialSelection(commerce) : []));
  const [quantity, setQuantity] = useState(1);
  const variant = commerce ? selectedVariant(commerce, selection) : null;
  const available = !commerce ? listing.stock : variant ? sellable(commerce, variant) : commerce.axes.length ? 0 : listingSellable(commerce, listing.stock);
  const max = commerce ? maxQuantity(commerce, available) : Math.min(listing.stock, 99);
  const choice: BuyChoice = { variant: variant?.id, quantity: max > 0 ? Math.min(Math.max(1, quantity), max) : 0 };
  const select = (next: Selection) => { setSelection(next); setQuantity(1); };
  return { selection, select, quantity: choice.quantity, setQuantity, choice, canBuy: max > 0, max };
}

/// Price, options and quantity for a version 7 listing, in place of the sats price.
export function ListingBuyTerms({ listing, buy }: { listing: ListingHeader; buy: ReturnType<typeof useBuyChoice> }) {
  const commerce = listing.commerce;
  if (!commerce || !listing.price) return null;
  const unit = basePrice(termsOf(listing), buy.choice.variant ?? NO_VARIANT);
  return <>
    <PriceTag commerce={commerce} price={listing.price} unit={unit} today={today()}/>
    <VariantPicker commerce={commerce} stock={listing.stock} selection={buy.selection} onSelect={buy.select} quantity={buy.quantity} onQuantity={buy.setQuantity} asOf={0}/>
  </>;
}

export function ListingBuyFacts({ listing }: { listing: ListingHeader }) {
  return listing.commerce && listing.price ? <ListingFacts commerce={listing.commerce} currency={listing.price.currency}/> : null;
}

/// Shipping service and coupon code for one package at checkout.
export function PackageTerms({ pkg, coupon, onService, onCoupon }: { pkg: CartPackage; coupon: string; onService: (index: number) => void; onCoupon: (code: string) => void }) {
  useListingStyles();
  if (!pkg.items.some(i => i.listing.commerce)) return null;
  const currency = pkg.quote?.currency ?? pkg.items[0].listing.price?.currency ?? "USD";
  const pickup = pkg.items.find(i => i.listing.commerce?.shipping.pickup)?.listing.commerce?.shipping.pickup ?? null;
  return <div className="ob-package-terms">
    {pkg.physical && (pkg.services.length > 0 || pickup) && <label className="ob-field">Shipping
      <select value={pkg.service} onChange={e => onService(Number(e.target.value))}>
        {pkg.services.map((s, i) => <option key={s.name} value={i}>{s.name} · {s.daysMin}–{s.daysMax} days · {s.price ? formatMinor(s.price, currency) : "Free"}</option>)}
        {pickup && <option value={-1}>Local pickup in {pickup}</option>}
      </select>
    </label>}
    {pkg.service === -1 && <p className="ob-pickup-note">You arrange the time and place with the seller in private messages. Meeting in person shows each of you to the other.</p>}
    <label className="ob-field">Coupon code (optional)
      <input value={coupon} maxLength={24} autoCapitalize="characters" onChange={e => onCoupon(e.target.value.toUpperCase())} placeholder="FALL-10"/>
    </label>
    {coupon && <p className="ob-shipping-note">The seller applies the code. You see the discount in their price before you pay.</p>}
    {pkg.quote && <p className="ob-shipping-note">Seller's price {formatMinor(pkg.quote.total, currency)} {pkg.service === -1 ? "for local pickup" : `including ${pkg.quote.shipping ? formatMinor(pkg.quote.shipping, currency) : "free"} shipping`}, paid in sats at the rate when you order.</p>}
    {pkg.problem && <p className="ob-form-error">{pkg.problem}</p>}
  </div>;
}
