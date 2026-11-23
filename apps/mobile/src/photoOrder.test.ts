import assert from "node:assert/strict";
import { test } from "node:test";

import { movePhoto } from "./photoOrder.ts";

test("moving a photo changes which one is the cover", () => {
  assert.deepEqual(movePhoto(["a", "b", "c"], 2, -1), ["a", "c", "b"]);
  assert.deepEqual(movePhoto(["a", "b", "c"], 0, 1), ["b", "a", "c"]);
  assert.deepEqual(movePhoto(["a", "b"], 0, -1), ["a", "b"]);
  assert.deepEqual(movePhoto(["a", "b"], 1, 1), ["a", "b"]);
});
