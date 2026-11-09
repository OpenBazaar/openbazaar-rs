import assert from "node:assert/strict";
import { test } from "node:test";

import { BULK_PATH, bulkEditBody, merchantBulkPrice } from "./bulkEdit.ts";

test("bulk edit signs selected listings and does not publish a draft", () => {
  const body = bulkEditBody(["listing-a"], { price: 1800 });
  assert.deepEqual(body, { ids: ["listing-a"], price: 1800 });
  assert.equal("draftId" in body, false);
  assert.equal(BULK_PATH, "/store/listings/bulk");
  assert.equal(BULK_PATH.includes("/store/drafts"), false);
});

test("bulk prices use merchant currency units and refuse mixed currencies", () => {
  assert.deepEqual(merchantBulkPrice("25.50", ["USD", "USD"]), { price: 2550 });
  assert.deepEqual(merchantBulkPrice("500", ["JPY"]), { price: 500 });
  assert.deepEqual(merchantBulkPrice("25000", ["SAT"]), { price: 25000 });
  assert.equal(typeof merchantBulkPrice("25", ["USD", "EUR"]), "string");
  assert.equal(typeof merchantBulkPrice("25", []), "string");
  assert.equal(typeof merchantBulkPrice("1.5", ["SAT"]), "string");
  assert.equal(typeof merchantBulkPrice("0", ["USD"]), "string");
  assert.equal(typeof merchantBulkPrice("9007199254740993", ["JPY"]), "string");
});
