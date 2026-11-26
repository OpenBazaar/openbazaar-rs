import type { ShopLocation } from "./shopHistory";

const GUEST_SECTIONS = new Set(["discover", "saved", "bag", "safety", "store"]);
const STORE_SECTIONS = new Set(["storefront", "listings", "coupons", "sales", "store"]);

/** Keep restored or deep-linked history entries on screens that work without a node. */
export function sanitizeGuestLocation(loc: ShopLocation, guest: boolean): ShopLocation {
  if (!guest) return loc;
  if (loc.page.name === "receipt" || loc.page.name === "order") {
    return { section: "discover", category: null, page: { name: "browse" } };
  }
  if (loc.page.name === "sale") {
    return { section: "store", category: null, page: { name: "browse" } };
  }
  if (GUEST_SECTIONS.has(loc.section)) return loc;
  if (STORE_SECTIONS.has(loc.section)) {
    return { section: "store", category: null, page: { name: "browse" } };
  }
  return { section: "discover", category: null, page: { name: "browse" } };
}
