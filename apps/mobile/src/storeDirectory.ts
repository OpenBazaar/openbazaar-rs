// The Store Directory page: which stores to show and in what order. Pure, so
// the page only draws what these return.
import type { StoreSummary } from "./records.ts";

export type DirectoryStoreFields = Pick<
  StoreSummary,
  "key" | "name" | "tagline" | "region" | "categories" | "listingCount" | "reviewCount" | "reviewAverage" | "acceptsLightning" | "acceptsXmr" | "acceptsZec"
>;

export type DirectoryScope = "all" | "following";
export type DirectoryRail = "all" | "btc" | "ln" | "xmr" | "zec";
export type DirectorySort = "listings" | "rating" | "name";

export type DirectoryOptions = {
  query: string;
  scope: DirectoryScope;
  /** Store keys this node follows. Used by the `following` scope. */
  followedIds: ReadonlySet<string>;
  /** `""` for every category. */
  category: string;
  rail: DirectoryRail;
  sort: DirectorySort;
};

export const DEFAULT_OPTIONS: Omit<DirectoryOptions, "followedIds"> = { query: "", scope: "all", category: "", rail: "all", sort: "listings" };

/** The rails a store signs for, as short labels. Every store takes Bitcoin. */
export function railLabels(store: Pick<StoreSummary, "acceptsLightning" | "acceptsXmr" | "acceptsZec">): string[] {
  return ["BTC", ...(store.acceptsLightning ? ["LN"] : []), ...(store.acceptsXmr ? ["XMR"] : []), ...(store.acceptsZec ? ["ZEC"] : [])];
}

function acceptsRail(store: DirectoryStoreFields, rail: DirectoryRail): boolean {
  if (rail === "ln") return store.acceptsLightning;
  if (rail === "xmr") return store.acceptsXmr;
  if (rail === "zec") return store.acceptsZec;
  return true;
}

/** Every category some store lists in, with how many stores list in it, by name. */
export function directoryCategories(stores: readonly Pick<StoreSummary, "categories">[]): { name: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const store of stores) {
    for (const name of new Set(store.categories)) {
      if (name) counts.set(name, (counts.get(name) ?? 0) + 1);
    }
  }
  return [...counts].map(([name, count]) => ({ name, count })).sort((a, b) => a.name.localeCompare(b.name));
}

function matchesQuery(store: DirectoryStoreFields, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  if (store.key.startsWith(needle)) return true;
  return [store.name, store.tagline, store.region, ...store.categories].some((text) => text.toLowerCase().includes(needle));
}

function byName(a: DirectoryStoreFields, b: DirectoryStoreFields): number {
  return a.name.localeCompare(b.name) || a.key.localeCompare(b.key);
}

const SORTS: Record<DirectorySort, (a: DirectoryStoreFields, b: DirectoryStoreFields) => number> = {
  listings: (a, b) => b.listingCount - a.listingCount || byName(a, b),
  // A store with no reviews has no rating, so it follows every rated store.
  rating: (a, b) => (b.reviewAverage ?? -1) - (a.reviewAverage ?? -1) || b.reviewCount - a.reviewCount || byName(a, b),
  name: byName,
};

/** The stores that pass every filter, in the chosen order. */
export function filterAndSortStores<T extends DirectoryStoreFields>(stores: readonly T[], options: DirectoryOptions): T[] {
  return stores
    .filter((store) => options.scope === "all" || options.followedIds.has(store.key))
    .filter((store) => !options.category || store.categories.includes(options.category))
    .filter((store) => acceptsRail(store, options.rail))
    .filter((store) => matchesQuery(store, options.query))
    .sort(SORTS[options.sort]);
}

/** True when any control narrows the list. */
export function filtersActive(options: Omit<DirectoryOptions, "followedIds">): boolean {
  return options.query.trim() !== "" || options.scope !== "all" || options.category !== "" || options.rail !== "all";
}

/** `4.8`, always one decimal. */
export function formatRating(average: number): string {
  return average.toFixed(1);
}

/** `12 items`, `1 item`, or `No listings yet`. */
export function listingCountLabel(count: number): string {
  return count <= 0 ? "No listings yet" : `${count.toLocaleString("en-US")} ${count === 1 ? "item" : "items"}`;
}

/** The first letter of a store's name, for a store with no avatar. */
export function monogram(name: string): string {
  return ([...name.trim()][0] ?? "?").toUpperCase();
}
