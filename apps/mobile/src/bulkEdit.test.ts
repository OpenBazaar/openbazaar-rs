import assert from "node:assert/strict";
import { test } from "node:test";

import { BULK_PATH, bulkEditBody } from "./bulkEdit.ts";

test("bulk edit signs selected listings and does not publish a draft", () => {
  const body = bulkEditBody(["listing-a"], { price: 1800 });
  assert.deepEqual(body, { ids: ["listing-a"], price: 1800 });
  assert.equal("draftId" in body, false);
  assert.equal(BULK_PATH, "/store/listings/bulk");
  assert.equal(BULK_PATH.includes("/store/drafts"), false);
});
