import type { ListingHeader } from "./catalog";

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

export function filterListings(
  listings: readonly ListingHeader[],
  query: string,
  also: readonly string[] = [],
): ListingHeader[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) {
    return [...listings];
  }
  return listings.filter((listing) =>
    [
      listing.title,
      listing.category,
      listing.shipFrom,
      listing.kind,
      listing.description ?? "",
      listing.seller,
      ...also,
    ].some((field) => field.toLowerCase().includes(needle)),
  );
}
