import assert from "node:assert/strict";
import { test } from "node:test";

import { followNotice, storesOf } from "./follows.ts";

const cedar = "aa".repeat(32);

test("the directory list keeps only rows with a store id, and added means followed", () => {
  assert.deepEqual(storesOf({ stores: [{ storeId: cedar, name: "Cedar Workshop", listings: 2, added: true }, { storeId: "bb", added: "yes" }, { name: "No id" }, null] }), [
    { storeId: cedar, name: "Cedar Workshop", listings: 2, added: true },
    { storeId: "bb", name: null, listings: 0, added: false },
  ]);
  for (const unreadable of [null, "stores", {}, { stores: "none" }]) assert.deepEqual(storesOf(unreadable), []);
});

test("the notice after a change says what the node now does with the store", () => {
  const row = { storeId: cedar, name: "Cedar Workshop", listings: 2 };
  assert.equal(followNotice([{ ...row, added: true }], cedar, "Cedar Workshop"), "Following Cedar Workshop. Your node keeps it even if the directory drops it.");
  assert.equal(followNotice([{ ...row, added: false }], cedar, "Cedar Workshop"), "Stopped following Cedar Workshop. The directory still names it.");
  assert.equal(followNotice([], cedar, "Cedar Workshop"), "Stopped following Cedar Workshop. Your node no longer shows it.");
});
