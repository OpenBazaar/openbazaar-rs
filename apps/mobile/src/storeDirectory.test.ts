import assert from "node:assert/strict";
import { test } from "node:test";

import {
  DEFAULT_OPTIONS,
  directoryCategories,
  filterAndSortStores,
  filtersActive,
  formatRating,
  listingCountLabel,
  monogram,
  railLabels,
  type DirectoryOptions,
  type DirectoryStoreFields,
} from "./storeDirectory.ts";

const key = (n: number) => String(n).repeat(64);
function store(n: number, over: Partial<DirectoryStoreFields> = {}): DirectoryStoreFields {
  return { key: key(n), name: `Store ${n}`, tagline: "", region: "", categories: [], listingCount: 0, reviewCount: 0, reviewAverage: null, acceptsLightning: false, acceptsXmr: false, acceptsZec: false, ...over };
}
const options = (over: Partial<DirectoryOptions> = {}): DirectoryOptions => ({ ...DEFAULT_OPTIONS, followedIds: new Set(), ...over });
const names = (stores: readonly DirectoryStoreFields[]) => stores.map((s) => s.name);

const marlow = store(1, { name: "Marlow Goods", tagline: "Books and paper from the coast", region: "Oregon", categories: ["Books", "Home"], listingCount: 12, reviewCount: 12, reviewAverage: 4.8, acceptsLightning: true });
const hale = store(2, { name: "Hale Paper", tagline: "Letterpress cards", region: "Vermont", categories: ["Paper", "Tools"], listingCount: 8, reviewCount: 3, reviewAverage: 4.3, acceptsXmr: true });
const north = store(3, { name: "North Kitchen", tagline: "Pantry staples", region: "Maine", categories: ["Grocery", "Home"], listingCount: 8, acceptsZec: true });
const empty = store(4, { name: "Another shop" });
const all = [empty, north, hale, marlow];

test("categories are unique, sorted, and count the stores that list in each", () => {
  assert.deepEqual(directoryCategories(all), [
    { name: "Books", count: 1 },
    { name: "Grocery", count: 1 },
    { name: "Home", count: 2 },
    { name: "Paper", count: 1 },
    { name: "Tools", count: 1 },
  ]);
  assert.deepEqual(directoryCategories([]), []);
  assert.deepEqual(directoryCategories([store(5, { categories: ["Home", "Home", ""] })]), [{ name: "Home", count: 1 }]);
});

test("the query matches name, tagline, region, category, or a key prefix, without case", () => {
  const found = (query: string) => names(filterAndSortStores(all, options({ query })));
  assert.deepEqual(found("marlow"), ["Marlow Goods"]);
  assert.deepEqual(found("LETTERPRESS"), ["Hale Paper"]);
  assert.deepEqual(found("vermont"), ["Hale Paper"]);
  assert.deepEqual(found("home"), ["Marlow Goods", "North Kitchen"]);
  assert.deepEqual(found("  tools "), ["Hale Paper"]);
  assert.deepEqual(found("333"), ["North Kitchen"]);
  assert.deepEqual(found("3333333333333x"), []);
  assert.equal(found("").length, 4);
  assert.deepEqual(found("zzz"), []);
});

test("a key is matched only from its start", () => {
  const keyed = [store(1, { key: "ab".repeat(32) })];
  assert.deepEqual(filterAndSortStores(keyed, options({ query: "bab" })), []);
  assert.equal(filterAndSortStores(keyed, options({ query: "ABAB" })).length, 1, "in any case");
});

test("the Following scope keeps only followed stores", () => {
  const followedIds = new Set([key(2), key(4)]);
  assert.deepEqual(names(filterAndSortStores(all, options({ scope: "following", followedIds }))), ["Hale Paper", "Another shop"]);
  assert.deepEqual(filterAndSortStores(all, options({ scope: "following" })), []);
  assert.equal(filterAndSortStores(all, options({ scope: "all", followedIds })).length, 4);
});

test("a category keeps the stores that list in it", () => {
  assert.deepEqual(names(filterAndSortStores(all, options({ category: "Home" }))), ["Marlow Goods", "North Kitchen"]);
  assert.deepEqual(filterAndSortStores(all, options({ category: "Hom" })), [], "the whole category name");
});

test("a rail keeps the stores that sign for it, and every store takes Bitcoin", () => {
  const on = (rail: DirectoryOptions["rail"]) => names(filterAndSortStores(all, options({ rail })));
  assert.equal(on("all").length, 4);
  assert.equal(on("btc").length, 4);
  assert.deepEqual(on("ln"), ["Marlow Goods"]);
  assert.deepEqual(on("xmr"), ["Hale Paper"]);
  assert.deepEqual(on("zec"), ["North Kitchen"]);
});

test("filters combine", () => {
  assert.deepEqual(names(filterAndSortStores(all, options({ category: "Home", rail: "zec", query: "pantry" }))), ["North Kitchen"]);
  assert.deepEqual(filterAndSortStores(all, options({ category: "Home", rail: "xmr" })), []);
});

test("sorting by listings puts the most first and breaks ties by name", () => {
  assert.deepEqual(names(filterAndSortStores(all, options({ sort: "listings" }))), ["Marlow Goods", "Hale Paper", "North Kitchen", "Another shop"]);
});

test("sorting by rating puts rated stores first, then more reviews, and unrated stores last by name", () => {
  const more = store(5, { name: "Aardvark Books", reviewCount: 9, reviewAverage: 4.3 });
  assert.deepEqual(names(filterAndSortStores([...all, more], options({ sort: "rating" }))), ["Marlow Goods", "Aardvark Books", "Hale Paper", "Another shop", "North Kitchen"]);
});

test("sorting by name is A to Z and does not change the list it was given", () => {
  const input = [...all];
  assert.deepEqual(names(filterAndSortStores(input, options({ sort: "name" }))), ["Another shop", "Hale Paper", "Marlow Goods", "North Kitchen"]);
  assert.deepEqual(input, all);
});

test("clear filters is offered when a control narrows the list", () => {
  assert.equal(filtersActive(DEFAULT_OPTIONS), false);
  assert.equal(filtersActive({ ...DEFAULT_OPTIONS, sort: "name" }), false, "a sort does not narrow");
  assert.equal(filtersActive({ ...DEFAULT_OPTIONS, query: "  " }), false);
  for (const over of [{ query: "x" }, { scope: "following" as const }, { category: "Home" }, { rail: "ln" as const }]) {
    assert.equal(filtersActive({ ...DEFAULT_OPTIONS, ...over }), true);
  }
});

test("labels read cleanly", () => {
  assert.equal(formatRating(4.8), "4.8");
  assert.equal(formatRating(5), "5.0");
  assert.equal(formatRating(4.25), "4.3");
  assert.equal(listingCountLabel(0), "No listings yet");
  assert.equal(listingCountLabel(1), "1 item");
  assert.equal(listingCountLabel(12), "12 items");
  assert.equal(listingCountLabel(1200), "1,200 items");
  assert.deepEqual(railLabels(marlow), ["BTC", "LN"]);
  assert.deepEqual(railLabels({ acceptsLightning: true, acceptsXmr: true, acceptsZec: true }), ["BTC", "LN", "XMR", "ZEC"]);
  assert.deepEqual(railLabels(empty), ["BTC"]);
  assert.equal(monogram(" marlow"), "M");
  assert.equal(monogram(""), "?");
});
