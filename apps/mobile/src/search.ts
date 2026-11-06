import type { ListingHeader } from "./catalog";

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
