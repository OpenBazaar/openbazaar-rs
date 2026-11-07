import assert from "node:assert/strict";
import { test } from "node:test";

import { applySync, collectionValue, emptyReplica, haveOf, type Replica } from "./replica.ts";

const listing = (hash: string, title: string) => ({ hash, title });

function first(): Replica {
  return applySync(emptyReplica(), {
    serverTime: 100,
    node: { nodeId: "n1", onion: "o", nextOnion: null, lan: [] },
    collections: {
      catalog: {
        doc: { etag: "c0", value: { stores: [{ id: "s" }], moderator: null } },
        upsert: [
          { id: "h1", etag: "e1", value: listing("h1", "Mug") },
          { id: "h2", etag: "e2", value: listing("h2", "Shirt") },
        ],
        remove: [],
      },
      wallet: { doc: { etag: "w0", value: { balance: { totalSats: 5 } } }, upsert: [], remove: [] },
    },
  }).replica;
}

test("a first sync takes every collection whole and asks for nothing it does not have", () => {
  assert.deepEqual(haveOf(emptyReplica()), {});
  const replica = first();
  assert.deepEqual(haveOf(replica), { catalog: { doc: "c0", items: { h1: "e1", h2: "e2" } }, wallet: { doc: "w0" } });
  assert.deepEqual(collectionValue(replica, "catalog"), { stores: [{ id: "s" }], moderator: null, listings: [listing("h1", "Mug"), listing("h2", "Shirt")] });
  assert.deepEqual(collectionValue(replica, "wallet"), { balance: { totalSats: 5 } });
  assert.equal(collectionValue(replica, "orders"), null);
  assert.deepEqual(replica.node, { nodeId: "n1", onion: "o", nextOnion: null, lan: [] });
});

test("add, change, and remove items; an unchanged doc is null", () => {
  const before = first();
  const { replica, changes } = applySync(before, {
    serverTime: 200,
    collections: {
      catalog: {
        doc: null,
        upsert: [
          { id: "h2", etag: "e2b", value: listing("h2", "Shirt, blue") },
          { id: "h3", etag: "e3", value: listing("h3", "Hat") },
        ],
        remove: ["h1"],
      },
    },
  });
  assert.deepEqual(changes.added, { catalog: ["h3"] });
  assert.deepEqual(changes.changed, { catalog: ["h2"] });
  assert.deepEqual(changes.removed, { catalog: ["h1"] });
  assert.equal(replica.collections.catalog.doc?.etag, "c0");
  assert.deepEqual(haveOf(replica).catalog, { doc: "c0", items: { h2: "e2b", h3: "e3" } });
  assert.deepEqual(collectionValue(replica, "catalog")?.listings, [listing("h2", "Shirt, blue"), listing("h3", "Hat")]);
  assert.equal(replica.collections.wallet.doc?.etag, "w0");
  assert.equal(before.collections.catalog.items.h1.etag, "e1");
});

test("remove applies before upsert, so an item removed and re-added stays", () => {
  const { replica } = applySync(first(), {
    collections: { catalog: { doc: null, upsert: [{ id: "h1", etag: "e1b", value: listing("h1", "Mug 2") }], remove: ["h1"] } },
  });
  assert.equal(replica.collections.catalog.items.h1.etag, "e1b");
});

test("a new doc replaces the old one with the node's etag", () => {
  const { replica } = applySync(first(), {
    collections: { wallet: { doc: { etag: "w1", value: { balance: { totalSats: 9 } } }, upsert: [], remove: [] } },
  });
  assert.deepEqual(replica.collections.wallet.doc, { etag: "w1", value: { balance: { totalSats: 9 } } });
});

test("a collection error leaves that collection's replica intact", () => {
  const before = first();
  const { replica, changes } = applySync(before, {
    serverTime: 300,
    collections: {
      wallet: { error: { status: 423, code: "locked" } },
      catalog: { doc: null, upsert: [], remove: [] },
    },
  });
  assert.deepEqual(changes.failed, ["wallet"]);
  assert.deepEqual(replica.collections.wallet.doc, before.collections.wallet.doc);
  assert.deepEqual(replica.collections.wallet.error, { status: 423, code: "locked" });
  assert.equal(replica.collections.wallet.syncedAt, 100);
  assert.deepEqual(haveOf(replica).wallet, { doc: "w0" });
  const recovered = applySync(replica, { collections: { wallet: { doc: null, upsert: [], remove: [] } } }).replica;
  assert.equal(recovered.collections.wallet.error, null);
});

test("an unreadable upsert leaves the collection intact; an unreadable answer changes nothing", () => {
  const before = first();
  const bad = applySync(before, { collections: { catalog: { doc: null, upsert: [{ id: "h9" }], remove: ["h1"] } } });
  assert.deepEqual(bad.replica.collections.catalog.items, before.collections.catalog.items);
  assert.deepEqual(bad.changes.failed, ["catalog"]);
  assert.equal(applySync(before, "nonsense").replica, before);
});

test("an upsert without an id takes the item's own id field", () => {
  const { replica } = applySync(emptyReplica(), {
    collections: { orders: { doc: { etag: "d", value: {} }, upsert: [{ etag: "o1", value: { id: "ord-1", total: 3 } }], remove: [] } },
  });
  assert.deepEqual(collectionValue(replica, "orders"), { orders: [{ id: "ord-1", total: 3 }] });
});
