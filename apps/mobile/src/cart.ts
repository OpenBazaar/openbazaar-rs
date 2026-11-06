import type { ListingHeader, ShopCatalog } from "./catalog";

export type CartLine = { hash: string; quantity: number };

export type CartItem = { listing: ListingHeader; quantity: number };

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
};

export function cartPackages(catalog: ShopCatalog, lines: CartLine[]): CartPackage[] {
  const groups = new Map<string, CartItem[]>();
  for (const line of lines) {
    const listing = catalog.listings.find((item) => item.hash === line.hash);
    if (!listing || line.quantity < 1) continue;
    const group = groups.get(listing.seller) ?? [];
    group.push({ listing, quantity: line.quantity });
    groups.set(listing.seller, group);
  }
  return [...groups.entries()].map(([seller, items]) => {
    const itemsSats = items.reduce((sum, item) => sum + item.listing.priceSats * item.quantity, 0);
    const physicalItems = items.filter((item) => item.listing.kind === "physical");
    const rate = physicalItems[0]?.listing.shippingSats ?? 0;
    const freeOverSats = physicalItems[0]?.listing.freeOverSats ?? 0;
    const shippingSats = physicalItems.length === 0 ? 0 : freeOverSats > 0 && itemsSats >= freeOverSats ? 0 : rate;
    return {
      seller,
      shipFrom: physicalItems[0]?.listing.shipFrom ?? items[0].listing.shipFrom,
      items,
      itemsSats,
      shippingSats,
      totalSats: itemsSats + shippingSats,
      freeOverSats: physicalItems.length > 0 ? freeOverSats : 0,
      physical: physicalItems.length > 0,
      digital: items.some((item) => item.listing.kind === "digital"),
      moderated: items.every((item) => item.listing.acceptsModerator) && catalog.moderator !== null,
    };
  });
}

export function alsoSoldBy(catalog: ShopCatalog, listing: ListingHeader): ListingHeader[] {
  if (!listing.product) return [];
  return catalog.listings.filter((other) => other.product === listing.product && other.hash !== listing.hash);
}
