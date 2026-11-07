import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { sha256Hex } from "./records.ts";
import { checkStoreImage, MAX_MEDIA_BYTES, parseStoreList, parseStorePage } from "./stores.ts";

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

test("a store page for another key, or with a broken record, is refused", () => {
  const key = String(raw().stores[1].key);
  assert.throws(() => parseStorePage(page(0), key));
  const broken = page(1);
  broken.store.record = String(broken.store.record).replace(/^4f/, "4e");
  assert.throws(() => parseStorePage(broken, key));
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
