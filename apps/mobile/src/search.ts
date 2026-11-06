import type { ListingHeader } from "./catalog";

export function filterListings(
  listings: readonly ListingHeader[],
  query: string,
): ListingHeader[] {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) {
    return [...listings];
  }
  return listings.filter(
    (listing) =>
      listing.title.toLowerCase().includes(needle) ||
      listing.category.toLowerCase().includes(needle),
  );
}
