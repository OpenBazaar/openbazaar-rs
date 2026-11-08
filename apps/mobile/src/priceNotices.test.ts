import assert from "node:assert/strict";
import { test } from "node:test";

import { priceDrops, rememberPrices, savedHashesAfterRefresh, sameWatches, type CatalogPrice, type WatchedListing } from "./priceNotices.ts";

const DROP_KEYS = ["after", "before", "currency", "listingId", "storeId", "title"];

function listing(overrides: Partial<CatalogPrice> = {}): CatalogPrice {
  return {
    listingId: "mug",
    storeId: "aa".repeat(32),
    hash: "11".repeat(32),
    title: "Stoneware mug",
    price: { amount: 2500, currency: "USD" },
    priceSats: 25_000,
    ...overrides,
  };
}

const watched: WatchedListing = {
  listingId: "mug",
  storeId: "aa".repeat(32),
  hash: "11".repeat(32),
  amount: 2500,
  currency: "USD",
};

test("a lower signed price is a notice with no buyer and nowhere to send it", () => {
  const drops = priceDrops([watched], [listing({ hash: "22".repeat(32), price: { amount: 2000, currency: "USD" }, priceSats: 20_000 })]);
  assert.equal(drops.length, 1);
  assert.deepEqual(Object.keys(drops[0]).sort(), DROP_KEYS);
  assert.deepEqual(drops[0], {
    listingId: "mug",
    storeId: watched.storeId,
    title: "Stoneware mug",
    before: 2500,
    after: 2000,
    currency: "USD",
  });
  const text = JSON.stringify(drops[0]);
  assert.equal(text.includes("buyer"), false);
  assert.equal(text.includes("address"), false);
  assert.equal(text.includes("http"), false);
});

test("the same price, a higher price, another currency, or another listing is not a notice", () => {
  assert.deepEqual(priceDrops([watched], [listing()]), []);
  assert.deepEqual(priceDrops([watched], [listing({ price: { amount: 3000, currency: "USD" } })]), []);
  assert.deepEqual(priceDrops([watched], [listing({ price: { amount: 1000, currency: "EUR" } })]), []);
  assert.deepEqual(priceDrops([watched], [listing({ listingId: "bowl", price: { amount: 1000, currency: "USD" } })]), []);
  assert.deepEqual(priceDrops([watched], []), []);
});

test("a sats listing is compared in sats, and remembering the new price stops a second notice", () => {
  const sats: WatchedListing = { ...watched, amount: 25_000, currency: "SAT" };
  const now = listing({ hash: "33".repeat(32), price: null, priceSats: 20_000 });
  const drops = priceDrops([sats], [now]);
  assert.equal(drops[0].currency, "SAT");
  assert.equal(drops[0].after, 20_000);
  const next = rememberPrices([sats], [now]);
  assert.equal(next[0].amount, 20_000);
  assert.equal(next[0].hash, now.hash);
  assert.deepEqual(priceDrops(next, [now]), []);
  assert.equal(sameWatches(next, rememberPrices(next, [now])), true);
});

test("a saved hash follows the listing's new signature on this device", () => {
  const now = listing({ hash: "44".repeat(32), price: { amount: 2000, currency: "USD" } });
  assert.deepEqual(savedHashesAfterRefresh([watched.hash, "ff".repeat(32)], [watched], [now]), [now.hash, "ff".repeat(32)]);
  assert.deepEqual(savedHashesAfterRefresh(["ff".repeat(32)], [watched], [now]), ["ff".repeat(32)]);
});
