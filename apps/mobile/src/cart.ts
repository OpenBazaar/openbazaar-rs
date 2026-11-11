import type { CurrencyCode, ListingHeader, RateTable, ShopCatalog } from "./catalog";
import type { ListingTerms, Service } from "./commerce.ts";
import { NO_VARIANT, quote, scaleOf, unitToday, type Quote } from "./pricing.ts";
import { listingSellable, sellable } from "./variants.ts";

/** `variant` is set for a version 7 listing with options; two variants are two lines. */
export type CartLine = { hash: string; quantity: number; variant?: string };

export type CartItem = {
  listing: ListingHeader;
  quantity: number;
  variant?: string;
  /** The bag's key for this line: listing hash and variant. */
  key: string;
  /** "Navy / M" for a variant. */
  label: string;
  unitSats: number;
  /** Units left as of the listing's last publish. */
  available: number;
};

export type CartPackage = {
  seller: string;
  shipFrom: string;
  items: CartItem[];
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  freeOverSats: number;
  physical: boolean;
  digital: boolean;
  moderated: boolean;
  /** Version 7 packages: the shipping services the listings offer, and the fiat quote for `service`. */
  services: Service[];
  service: number;
  quote: Quote | null;
  /** Why the package cannot be priced as it stands. */
  problem: string;
};

export function lineKey(line: { hash: string; variant?: string }): string {
  return `${line.hash}:${line.variant ?? ""}`;
}

export function termsOf(listing: ListingHeader): ListingTerms {
  return { version: listing.commerce ? 7 : 6, storeId: listing.storeId, kind: listing.kind, stock: listing.stock, listingId: listing.listingId, images: listing.images, price: listing.price, commerce: listing.commerce ?? null };
}

/** Sats for one minor unit of a listing's fiat price. The node charges at its
 *  rate when the order is placed, so `rates` comes first; without one this is
 *  the rate the listing was signed with. */
export function satsPerMinor(listing: ListingHeader, rates: RateTable): number {
  const price = listing.price;
  if (!price || price.amount <= 0) return 0;
  const btc = rates[price.currency as CurrencyCode];
  return btc ? 1e8 / (btc * scaleOf(price.currency)) : listing.priceSats / price.amount;
}

function itemOf(listing: ListingHeader, line: CartLine, rates: RateTable): CartItem {
  const commerce = listing.commerce ?? null;
  const variant = commerce?.variants.find(v => v.id === line.variant);
  const available = variant && commerce ? sellable(commerce, variant) : listingSellable(commerce, listing.stock);
  const ratio = satsPerMinor(listing, rates);
  const unitSats = commerce && ratio ? Math.round(unitToday(termsOf(listing), line.variant ?? NO_VARIANT, today()) * ratio) : listing.priceSats;
  return { listing, quantity: line.quantity, variant: line.variant, key: lineKey(line), label: variant ? variant.options.join(" / ") : "", unitSats, available };
}

function today(): number {
  const now = new Date();
  return now.getUTCFullYear() * 10_000 + (now.getUTCMonth() + 1) * 100 + now.getUTCDate();
}

/** The fiat quote for a package with version 7 listings, in sats as `satsPerMinor` converts it. */
function priced(items: CartItem[], service: number, rates: RateTable): Pick<CartPackage, "quote" | "problem" | "itemsSats" | "shippingSats"> | null {
  if (!items.some(i => i.listing.commerce)) return null;
  const ratio = satsPerMinor(items[0].listing, rates);
  const lines = items.map(i => ({ listing: termsOf(i.listing), variant: i.variant ?? NO_VARIANT, quantity: i.quantity, delivery: i.listing.kind === "digital" ? "digital" as const : service < 0 ? "pickup" as const : { service } }));
  const result = quote(lines, null, { today: today(), now: Math.floor(Date.now() / 1000) });
  if (!result.ok) return { quote: null, problem: result.message, itemsSats: items.reduce((n, i) => n + i.unitSats * i.quantity, 0), shippingSats: 0 };
  return { quote: result.quote, problem: "", itemsSats: Math.round(result.quote.items * ratio), shippingSats: Math.round(result.quote.shipping * ratio) };
}

/** `services` maps a seller to the chosen shipping service index (-1 is local pickup).
 *  `rates` is the node's rate table, for packages priced in a fiat currency. */
export function cartPackages(catalog: ShopCatalog, lines: CartLine[], services: Record<string, number> = {}, rates: RateTable = {}): CartPackage[] {
  const groups = new Map<string, CartItem[]>();
  for (const line of lines) {
    const listing = catalog.listings.find((item) => item.hash === line.hash);
    if (!listing || line.quantity < 1) continue;
    const group = groups.get(listing.seller) ?? [];
    group.push(itemOf(listing, line, rates));
    groups.set(listing.seller, group);
  }
  return [...groups.entries()].map(([seller, items]) => {
    const physicalItems = items.filter((item) => item.listing.kind === "physical");
    const offered = physicalItems.map(i => i.listing.commerce?.shipping.services ?? []).find(s => s.length) ?? [];
    const service = services[seller] ?? 0;
    const v7 = priced(items, service, rates);
    const plainItems = items.reduce((sum, item) => sum + item.unitSats * item.quantity, 0);
    const rate = physicalItems[0]?.listing.shippingSats ?? 0;
    const freeOverSats = physicalItems[0]?.listing.freeOverSats ?? 0;
    const plainShipping = physicalItems.length === 0 ? 0 : freeOverSats > 0 && plainItems >= freeOverSats ? 0 : rate;
    const itemsSats = v7 ? v7.itemsSats : plainItems;
    const shippingSats = v7 ? v7.shippingSats : plainShipping;
    return {
      seller,
      shipFrom: physicalItems[0]?.listing.shipFrom ?? items[0].listing.shipFrom,
      items,
      itemsSats,
      shippingSats,
      totalSats: itemsSats + shippingSats,
      freeOverSats: physicalItems.length > 0 && !v7 ? freeOverSats : 0,
      physical: physicalItems.length > 0,
      digital: items.some((item) => item.listing.kind === "digital"),
      moderated: items.every((item) => item.listing.acceptsModerator) && catalog.moderator !== null,
      services: offered,
      service,
      quote: v7?.quote ?? null,
      problem: v7?.problem ?? "",
    };
  });
}

export function alsoSoldBy(catalog: ShopCatalog, listing: ListingHeader): ListingHeader[] {
  if (!listing.product) return [];
  return catalog.listings.filter((other) => other.product === listing.product && other.hash !== listing.hash);
}
