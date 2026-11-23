import type { ListingHeader } from "./catalog";
import { CONDITIONS, type Condition } from "./commerce.ts";
import { SPECIFIC_KEYS, sameChoice } from "./listingFields.ts";
import { displayText } from "./safeText.ts";

export type StoreSearch = {
  key: string;
  name: string;
  region: string;
  tagline: string;
  about?: string;
};

/// Name, region, tagline, and about. A store matches before any listing is held.
export function matchingStores<T extends StoreSearch>(stores: readonly T[], query: string): T[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return [];
  return stores.filter((store) =>
    [store.name, store.region, store.tagline, store.about ?? ""].some((field) => field.toLowerCase().includes(needle)),
  );
}

/// Stores whose record is held and whose listings are not.
export function storesWithoutListings<T extends { key: string }>(
  stores: readonly T[],
  listings: readonly { storeId: string }[],
): T[] {
  const held = new Set(listings.map((listing) => listing.storeId));
  return stores.filter((store) => !held.has(store.key));
}

/// Price bounds and signed version-7 fields. Empty or unknown values do not narrow.
export type ListingNarrowing = {
  min?: string;
  max?: string;
  condition?: string;
  specifics?: Record<string, string>;
};

const MAX_SPECIFIC = 120;

function knownCondition(value: unknown): Condition | null {
  return typeof value === "string" && CONDITIONS.some((item) => item === value) ? (value as Condition) : null;
}

/// A sats bound from a price field. Anything that is not a plain number is no bound.
function satsBound(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (!text || text.length > 16 || !/^\d+(\.\d+)?$/.test(text)) return null;
  const amount = Number(text);
  return Number.isFinite(amount) ? amount : null;
}

function withinPrice(listing: ListingHeader, min: number | null, max: number | null): boolean {
  if (min === null && max === null) return true;
  const price = listing.priceSats;
  if (typeof price !== "number" || !Number.isFinite(price)) return false;
  if (min !== null && price < min) return false;
  if (max !== null && price > max) return false;
  return true;
}

/// Own string values only. Inherited names such as `constructor` are not a specific.
function signedSpecific(specifics: object, canonical: string): string | null {
  for (const key of Object.keys(specifics)) {
    if (!sameChoice(canonical, key) || !Object.hasOwn(specifics, key)) continue;
    const value = (specifics as Record<string, unknown>)[key];
    return typeof value === "string" ? value : null;
  }
  return null;
}

/// Known item specifics the buyer actually set. Unknown keys, including prototype names, are dropped.
export function chosenSpecifics(specifics: Record<string, string> | undefined): [string, string][] {
  if (!specifics || typeof specifics !== "object") return [];
  const chosen: [string, string][] = [];
  for (const key of Object.keys(specifics)) {
    const canonical = SPECIFIC_KEYS.find((name) => sameChoice(name, key));
    if (!canonical || !Object.hasOwn(specifics, key)) continue;
    const value = specifics[key];
    if (typeof value !== "string" || !value) continue;
    chosen.push([canonical, value]);
  }
  return chosen;
}

export function setSpecificChoice(current: Record<string, string> | undefined, key: string, value: string): Record<string, string> {
  if (!SPECIFIC_KEYS.some((name) => name === key)) return { ...Object.fromEntries(chosenSpecifics(current)) };
  const next: Record<string, string> = {};
  for (const [name, kept] of chosenSpecifics(current)) {
    if (name !== key) next[name] = kept;
  }
  if (value) next[key] = value;
  return next;
}

function matchesSpecifics(listing: ListingHeader, chosen: [string, string][]): boolean {
  if (chosen.length === 0) return true;
  const specifics = listing.commerce?.specifics;
  if (!specifics || typeof specifics !== "object") return false;
  return chosen.every(([key, wanted]) => signedSpecific(specifics, key) === wanted);
}

/// How many of the signed-field filters are set. Seller, kind and origin are counted by the caller.
export function narrowingActive(narrowing: ListingNarrowing): number {
  const prices = [narrowing.min, narrowing.max].filter((value) => satsBound(value) !== null).length;
  return prices + (knownCondition(narrowing.condition) ? 1 : 0) + chosenSpecifics(narrowing.specifics).length;
}

/// Conditions signed onto the listings, in the fixed order. Other text is not a choice.
export function conditionChoices(listings: readonly { commerce?: { condition?: string } | null }[]): Condition[] {
  const present = new Set<string>();
  for (const listing of listings) {
    const condition = knownCondition(listing.commerce?.condition);
    if (condition) present.add(condition);
  }
  return CONDITIONS.filter((condition) => present.has(condition));
}

export type SpecificChoice = { key: string; label: string; values: { signed: string; label: string }[] };

/// Item specifics a buyer can filter by. Only the fixed keys, with values cleaned for display.
/// A key the listing invented does not become a filter.
export function specificChoices(listings: readonly { commerce?: { specifics?: Record<string, string> } | null }[]): SpecificChoice[] {
  const byKey = new Map<string, Map<string, string>>();
  for (const listing of listings) {
    const specifics = listing.commerce?.specifics;
    if (!specifics || typeof specifics !== "object") continue;
    for (const key of Object.keys(specifics)) {
      const canonical = SPECIFIC_KEYS.find((name) => sameChoice(name, key));
      if (!canonical || !Object.hasOwn(specifics, key)) continue;
      const signed = specifics[key];
      if (typeof signed !== "string" || signed.length === 0 || signed.length > MAX_SPECIFIC) continue;
      const label = displayText(signed, 80);
      if (!label) continue;
      const values = byKey.get(canonical) ?? new Map<string, string>();
      if (!values.has(signed)) values.set(signed, label);
      byKey.set(canonical, values);
    }
  }
  return SPECIFIC_KEYS.filter((key) => byKey.has(key)).map((key) => ({
    key,
    label: key,
    values: [...(byKey.get(key)?.entries() ?? [])].map(([signed, label]) => ({ signed, label })).sort((a, b) => a.label.localeCompare(b.label)),
  }));
}

export function filterListings(
  listings: readonly ListingHeader[],
  query: string,
  also: readonly string[] = [],
  narrowing: ListingNarrowing = {},
): ListingHeader[] {
  const needle = query.trim().toLowerCase();
  const min = satsBound(narrowing.min);
  const max = satsBound(narrowing.max);
  const condition = knownCondition(narrowing.condition);
  const specifics = chosenSpecifics(narrowing.specifics);
  return listings.filter((listing) => {
    const text = needle.length === 0 || [
      listing.title,
      listing.category,
      listing.shipFrom,
      listing.kind,
      listing.description ?? "",
      listing.seller,
      ...also,
    ].some((field) => field.toLowerCase().includes(needle));
    return text && withinPrice(listing, min, max) && (!condition || listing.commerce?.condition === condition) && matchesSpecifics(listing, specifics);
  });
}
