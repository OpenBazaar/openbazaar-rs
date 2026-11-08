// A price-drop notice is computed on the buyer's device from prices that
// device already stored. The seller is never told who watches. A notice
// names a listing and two prices. It does not name a buyer, an address,
// or anywhere to send the notice.

export type WatchedListing = {
  listingId: string;
  storeId: string;
  hash: string;
  amount: number;
  currency: string;
};

export type CatalogPrice = {
  listingId: string | null;
  storeId: string;
  hash: string;
  title: string;
  price: { amount: number; currency: string } | null;
  priceSats: number;
};

export type PriceDrop = {
  listingId: string;
  storeId: string;
  title: string;
  before: number;
  after: number;
  currency: string;
};

export type SignedPrice = { amount: number; currency: string };

/// The price signed on the listing: its fiat price, or the sats price when it has none.
export function signedPrice(listing: CatalogPrice): SignedPrice | null {
  const fiat = listing.price;
  if (fiat && fiat.currency && Number.isSafeInteger(fiat.amount) && fiat.amount >= 0) {
    return { amount: fiat.amount, currency: fiat.currency };
  }
  if (Number.isSafeInteger(listing.priceSats) && listing.priceSats >= 0) return { amount: listing.priceSats, currency: "SAT" };
  return null;
}

function current(watched: WatchedListing, listings: readonly CatalogPrice[]): CatalogPrice | undefined {
  return listings.find((listing) => listing.listingId === watched.listingId && listing.storeId === watched.storeId);
}

/// Notices for watched listings whose signed price is lower, in the same currency.
export function priceDrops(watched: readonly WatchedListing[], listings: readonly CatalogPrice[]): PriceDrop[] {
  const drops: PriceDrop[] = [];
  for (const watch of watched) {
    const listing = current(watch, listings);
    const now = listing ? signedPrice(listing) : null;
    if (!listing || !now || now.currency !== watch.currency || !Number.isSafeInteger(watch.amount)) continue;
    if (now.amount < watch.amount) {
      drops.push({
        listingId: watch.listingId,
        storeId: watch.storeId,
        title: listing.title,
        before: watch.amount,
        after: now.amount,
        currency: now.currency,
      });
    }
  }
  return drops;
}

/// The prices to remember after a refresh. Still only on this device.
export function rememberPrices(watched: readonly WatchedListing[], listings: readonly CatalogPrice[]): WatchedListing[] {
  return watched.map((watch) => {
    const listing = current(watch, listings);
    if (!listing) return watch;
    const now = signedPrice(listing);
    if (!now || now.currency !== watch.currency) return { ...watch, hash: listing.hash };
    return { listingId: watch.listingId, storeId: watch.storeId, hash: listing.hash, amount: now.amount, currency: now.currency };
  });
}

export function sameWatches(left: readonly WatchedListing[], right: readonly WatchedListing[]): boolean {
  return left.length === right.length && left.every((watch, index) => {
    const other = right[index];
    return watch.listingId === other.listingId && watch.storeId === other.storeId && watch.hash === other.hash && watch.amount === other.amount && watch.currency === other.currency;
  });
}

/// Saved content hashes, pointed at the new signature of a watched listing.
export function savedHashesAfterRefresh(saved: readonly string[], watched: readonly WatchedListing[], listings: readonly CatalogPrice[]): string[] {
  return saved.map((hash) => {
    const watch = watched.find((item) => item.hash === hash);
    if (!watch) return hash;
    return current(watch, listings)?.hash ?? hash;
  });
}
