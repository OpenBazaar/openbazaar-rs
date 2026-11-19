import assert from "node:assert/strict";
import { test } from "node:test";

import { inventoryCount } from "./inventoryCount.ts";

test("counts published listings and drafts separately", () => {
  assert.equal(inventoryCount(1, 1, 1), "1 published · 1 draft");
  assert.equal(inventoryCount(3, 3, 2), "3 published · 2 drafts");
  assert.equal(inventoryCount(4, 4, 0), "4 published");
});

test("a filter shows how many published listings match", () => {
  assert.equal(inventoryCount(5, 2, 1), "2 of 5 published · 1 draft");
  assert.equal(inventoryCount(5, 0, 0), "0 of 5 published");
});
