import assert from "node:assert/strict";
import { test } from "node:test";

import type { ListingHeader } from "../catalog.ts";
import { ShopHistory, type HistoryLike, type ShopLocation } from "./shopHistory.ts";

class MemoryHistory implements HistoryLike {
  entries: unknown[] = [null];
  index = 0;
  listener: ((state: unknown) => void) | null = null;
  get state() { return this.entries[this.index]; }
  pushState(data: unknown) {
    this.index += 1;
    this.entries.splice(this.index, Infinity, data);
  }
  replaceState(data: unknown) { this.entries[this.index] = data; }
  back() { this.go(-1); }
  go(delta: number) {
    const next = this.index + delta;
    if (next < 0 || next >= this.entries.length) return;
    this.index = next;
    this.listener?.(this.entries[this.index]);
  }
}

function item(hash: string, storeId = "store-a"): ListingHeader {
  return {
    hash, storeId, title: hash, category: "Home", priceSats: 1, shipFrom: "US", kind: "physical",
    acceptsModerator: false, acceptsXmr: false, acceptsLightning: false, seller: "Ada", stock: 1,
    listedOn: 1, product: "p", shippingSats: 0, freeOverSats: 0, contentRating: "general",
    contentTags: [], listingId: null, images: [], price: null,
  };
}

function browse(section: string, category: string | null = null): ShopLocation {
  return { section, category, page: { name: "browse" } };
}
function store(id: string): ShopLocation {
  return { section: "discover", category: null, page: { name: "store", storeId: id } };
}
function detail(hash: string, fromStore?: string): ShopLocation {
  return { section: "discover", category: null, page: fromStore ? { name: "detail", listing: item(hash, fromStore), fromStore } : { name: "detail", listing: item(hash) } };
}

function harness() {
  const memory = new MemoryHistory();
  const shop = new ShopHistory(memory);
  const seen: ShopLocation[] = [];
  memory.listener = state => shop.onPop(state);
  shop.listen(loc => seen.push(loc));
  shop.start(browse("discover"));
  return { memory, shop, seen };
}

function last(seen: ShopLocation[]): ShopLocation {
  const loc = seen.at(-1);
  assert.ok(loc);
  return loc;
}

test("store then listing walks back to the store, then to products", () => {
  const { memory, shop, seen } = harness();
  shop.page(store("store-a"));
  shop.page(detail("mug", "store-a"));
  shop.back();
  assert.equal(last(seen).page.name, "store");
  shop.back();
  assert.equal(last(seen).page.name, "browse");
  assert.equal(last(seen).section, "discover");
  assert.equal(memory.index, 0);
  memory.go(1);
  assert.equal(last(seen).page.name, "store");
  memory.go(1);
  assert.equal(last(seen).page.name, "detail");
});

test("a product opened from products goes back to products", () => {
  const { shop, seen } = harness();
  shop.page(detail("mug"));
  shop.back();
  assert.equal(last(seen).page.name, "browse");
});

test("opening a store from a product returns to products, not the product", () => {
  const { memory, shop, seen } = harness();
  shop.page(detail("mug"));
  shop.page(store("store-a"));
  assert.equal(memory.entries.length, 2);
  shop.back();
  assert.equal(last(seen).page.name, "browse");
});

test("the store link on its own product steps back onto that store", () => {
  const { memory, shop, seen } = harness();
  shop.page(store("store-a"));
  shop.page(detail("mug", "store-a"));
  shop.page(store("store-a"));
  assert.equal(memory.index, 1);
  assert.equal(last(seen).page.name, "store");
  shop.back();
  assert.equal(last(seen).page.name, "browse");
  memory.go(1);
  assert.equal(last(seen).page.name, "store");
  memory.go(1);
  assert.equal(last(seen).page.name, "detail");
});

test("a different store from a product lands on products when going back", () => {
  const { memory, shop, seen } = harness();
  shop.page(store("store-a"));
  shop.page(detail("mug", "store-a"));
  shop.page(store("store-b"));
  assert.equal(memory.entries.length, 2);
  assert.equal(last(seen).page.name, "store");
  shop.back();
  assert.equal(last(seen).page.name, "browse");
});

test("another seller replaces the product so back returns to products", () => {
  const { memory, shop, seen } = harness();
  shop.page(detail("one"));
  shop.page(detail("two"));
  assert.equal(memory.entries.length, 2);
  shop.back();
  assert.equal(last(seen).page.name, "browse");
});

test("a section change pushes, and back from the root does not trap the shop", () => {
  const { memory, shop, seen } = harness();
  shop.section(browse("saved"));
  shop.section(browse("saved"));
  assert.equal(memory.entries.length, 2);
  shop.back();
  assert.equal(last(seen).section, "discover");
  shop.back();
  assert.equal(memory.index, 0);
  assert.equal(memory.entries.length, 2);
});

test("back from a section returns to the product underneath it", () => {
  const { shop, seen } = harness();
  shop.page(detail("mug"));
  shop.section(browse("wallet"));
  shop.back();
  assert.equal(last(seen).page.name, "detail");
});

test("checkout back returns to the product, and a receipt skips checkout", () => {
  const { shop, seen } = harness();
  const listing = item("mug");
  shop.page({ section: "discover", category: null, page: { name: "detail", listing } });
  shop.page({ section: "discover", category: null, page: { name: "checkout", lines: [{ hash: "mug", quantity: 1 }], returnTo: listing } });
  shop.back();
  assert.equal(last(seen).page.name, "detail");
  shop.page({ section: "discover", category: null, page: { name: "checkout", lines: [{ hash: "mug", quantity: 1 }], returnTo: listing } });
  shop.replace({ section: "discover", category: null, page: { name: "receipt", receipt: { checkoutId: "c", itemsSats: 1, shippingSats: 0, totalSats: 1, orders: [] } } });
  shop.back();
  assert.equal(last(seen).page.name, "detail");
});

test("bag checkout goes back to the bag", () => {
  const { shop, seen } = harness();
  shop.section(browse("bag"));
  shop.page({ section: "bag", category: null, page: { name: "checkout", lines: [{ hash: "mug", quantity: 2 }] } });
  shop.back();
  assert.equal(last(seen).section, "bag");
  assert.equal(last(seen).page.name, "browse");
});

test("hiding a listing returns to the products under the store", () => {
  const { memory, shop, seen } = harness();
  shop.page(store("store-a"));
  shop.page(detail("mug", "store-a"));
  shop.backToBrowse();
  assert.equal(memory.index, 0);
  assert.equal(last(seen).page.name, "browse");
});

test("a category chosen before opening a product is restored on back", () => {
  const { shop, seen } = harness();
  shop.page({ section: "discover", category: "Books", page: { name: "detail", listing: item("novel") } });
  shop.back();
  const loc = last(seen);
  assert.equal(loc.page.name, "browse");
  assert.equal(loc.category, "Books");
});

test("a pop from outside the shop does not rewrite the open page", () => {
  const { shop, seen } = harness();
  shop.page(store("store-a"));
  shop.onPop(null);
  shop.onPop({ ob: "other" });
  assert.equal(last(seen).page.name, "store");
});
