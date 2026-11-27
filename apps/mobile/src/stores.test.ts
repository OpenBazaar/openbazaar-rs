import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { sha256Hex } from "./records.ts";
import { checkStoreImage, MAX_MEDIA_BYTES, parseStoreList, parseStorePage, storefrontImages } from "./stores.ts";

type Raw = { stores: Record<string, unknown>[]; listings: Record<string, unknown>[]; moderator: { id: string; name: string } };
const raw = (): Raw => JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url), "utf8")) as Raw;

function page(index: number) {
  const catalog = raw();
  const store = catalog.stores[index];
  return {
    store: { ...store, about: "ignored: the record is shown" },
    listings: catalog.listings,
    moderators: [{ id: catalog.moderator.id, name: catalog.moderator.name }],
  };
}

test("the store list keeps only stores whose record verifies", () => {
  const catalog = raw();
  assert.equal(parseStoreList({ stores: catalog.stores }).stores.length, 3);
  const broken = catalog.stores.map((store, i) => (i === 1 ? { ...store, record: String(store.record).replace(/.$/, "0") } : store));
  const list = parseStoreList({ stores: [...broken, { key: "ab", name: "No record" }] });
  assert.deepEqual(list.stores.map((store) => store.name), ["Marlow Goods", "North Kitchen"]);
  assert.equal(list.rejected, 2);
  const renamed = parseStoreList({ stores: [{ ...catalog.stores[0], name: "Someone else", key: catalog.stores[0].key }] });
  assert.equal(renamed.stores[0].name, "Marlow Goods", "the name is the signed one");
});

test("a store page takes its profile from the signed record and only that store's listings", () => {
  const key = String(raw().stores[1].key);
  const store = parseStorePage(page(1), key);
  assert.equal(store.store.name, "Hale Paper");
  assert.deepEqual(store.store.shipsTo, ["US"]);
  assert.equal(store.store.createdOn, 20260120);
  assert.notEqual(store.store.about, "ignored: the record is shown");
  assert.ok(store.store.policies.shipping && store.store.policies.returns && store.store.policies.processingTime);
  assert.ok(store.store.banner && store.store.avatar);
  assert.equal(store.store.onion, null);
  assert.ok(store.listings.length > 0 && store.listings.every((listing) => listing.storeId === key));
  assert.equal(store.rejected, raw().listings.length - store.listings.length);
  assert.deepEqual(store.moderators, [{ id: raw().moderator.id, name: raw().moderator.name }]);
  assert.equal(store.store.acceptsXmr, false);
  assert.equal(store.store.unpublishedChanges, false);
  const claimed = page(1);
  Object.assign(claimed.store, { unpublishedChanges: true, acceptsXmr: true });
  const parsed = parseStorePage(claimed, key).store;
  assert.equal(parsed.unpublishedChanges, false, "only the node's own store can have unpublished changes");
  assert.equal(parsed.acceptsXmr, false, "the Monero flag is the signed one");
});

test("a listing the catalog index leaves out is dropped, and a bad signature already is", () => {
  const key = String(raw().stores[1].key);
  const full = parseStorePage(page(1), key);
  assert.ok(full.listings.length > 1);
  assert.equal(full.catalogIndex, null);
  const keep = full.listings[0].hash;
  const narrowed = parseStorePage({ ...page(1), catalogIndex: [keep, "not-a-hash"] }, key);
  assert.equal(narrowed.catalogIndex, null, "a catalog index that is not all hashes is ignored");
  const indexed = parseStorePage({ ...page(1), catalogIndex: [keep] }, key);
  assert.deepEqual(indexed.listings.map((listing) => listing.hash), [keep]);
  assert.equal(indexed.rejected, full.rejected + full.listings.length - 1);
  const broken = page(1);
  broken.listings = [...broken.listings, { ...broken.listings[0], record: String(broken.listings[0].record).replace(/.$/, "0") }];
  const checked = parseStorePage(broken, key);
  assert.equal(checked.listings.length, full.listings.length);
  assert.ok(checked.rejected > full.rejected);
});

test("a store page for another key, or with a broken record, is refused", () => {
  const key = String(raw().stores[1].key);
  assert.throws(() => parseStorePage(page(0), key));
  const broken = page(1);
  broken.store.record = String(broken.store.record).replace(/^4f/, "4e");
  assert.throws(() => parseStorePage(broken, key));
});

test("the storefront shows signed image hashes and ignores anything that is not one", () => {
  const avatar = "ab".repeat(32);
  const banner = "cd".repeat(32);
  assert.deepEqual(storefrontImages({ avatar, banner }), { avatar, banner });
  assert.deepEqual(storefrontImages({ details: { avatar, banner } }), { avatar, banner });
  assert.deepEqual(storefrontImages({ avatar: null, details: { avatar, banner: null } }), { avatar, banner: null });
  assert.deepEqual(storefrontImages({ avatar: "not a hash", banner: "00", details: { avatar: null, banner } }), { avatar: null, banner });
  assert.deepEqual(storefrontImages({}), { avatar: null, banner: null });
});

test("an image is returned only when its bytes hash to the signed hash", () => {
  const png = new Uint8Array(readFileSync(new URL("../../../crates/protocol/fixtures/media/hale-paper-avatar.png", import.meta.url)));
  const hash = sha256Hex(png);
  assert.equal(hash, parseStorePage(page(1), String(raw().stores[1].key)).store.avatar);
  assert.deepEqual(checkStoreImage(hash, "image/png", png), { contentType: "image/png", bytes: png });
  const changed = new Uint8Array(png);
  changed[png.length - 1] ^= 1;
  assert.equal(checkStoreImage(hash, "image/png", changed), null);
  assert.equal(checkStoreImage(hash, "text/html", png), null);
  const big = new Uint8Array(MAX_MEDIA_BYTES + 1);
  assert.equal(checkStoreImage(sha256Hex(big), "image/png", big), null);
});

test("a store list carries the directory fields, and an older node's list gets clean defaults", () => {
  const catalog = raw();
  const enriched = parseStoreList({
    stores: [{ ...catalog.stores[0], categories: ["Home", "Books", "Home", 7, "  "], reviewCount: 12, reviewAverage: 4.8 }],
  }).stores[0];
  assert.deepEqual(enriched.categories, ["Books", "Home"]);
  assert.equal(enriched.reviewCount, 12);
  assert.equal(enriched.reviewAverage, 4.8);
  assert.ok(enriched.banner, "the banner is the signed one");
  assert.equal(enriched.acceptsLightning, false, "the Lightning flag is the signed one");

  const older = parseStoreList({ stores: catalog.stores }).stores[0];
  assert.deepEqual([older.categories, older.reviewCount, older.reviewAverage], [[], 0, null]);

  for (const bad of [{ reviewCount: -1, reviewAverage: 5 }, { reviewCount: 1.5, reviewAverage: 5 }, { reviewCount: 2, reviewAverage: 9 }, { reviewCount: 0, reviewAverage: 4 }, { reviewCount: "3", reviewAverage: "4" }, { categories: "Home" }]) {
    const parsed = parseStoreList({ stores: [{ ...catalog.stores[0], ...bad }] }).stores[0];
    assert.ok(parsed.reviewAverage === null || (parsed.reviewCount > 0 && parsed.reviewAverage >= 1), JSON.stringify(bad));
  }
  const forged = parseStoreList({ stores: [{ ...catalog.stores[0], acceptsLightning: true, banner: "00".repeat(32) }] }).stores[0];
  assert.equal(forged.acceptsLightning, false);
  assert.notEqual(forged.banner, "00".repeat(32));
});
