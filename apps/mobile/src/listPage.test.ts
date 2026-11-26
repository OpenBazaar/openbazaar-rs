import assert from "node:assert/strict";
import { test } from "node:test";

import { listPage } from "./listPage.ts";

test("list pages stay bounded and cover every row once", () => {
  const rows = Array.from({ length: 100 }, (_, i) => i);
  const seen: number[] = [];
  for (let page = 0; page < 4; page++) {
    const slice = listPage(rows, page, 25);
    assert.equal(slice.pages, 4);
    assert.equal(slice.page, page);
    assert.equal(slice.items.length, 25);
    seen.push(...slice.items);
  }
  assert.deepEqual(seen, rows);
  assert.equal(listPage(rows, 99, 25).page, 3);
  assert.equal(listPage([], 5, 25).page, 0);
  assert.deepEqual(listPage([], 0, 25).items, []);
});
