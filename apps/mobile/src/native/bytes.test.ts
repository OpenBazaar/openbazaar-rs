import assert from "node:assert/strict";
import { test } from "node:test";

import { bodyBytes, responseFromAnswer } from "./bytes.ts";

test("an image answer keeps every byte", async () => {
  const png = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xd8]);
  const response = responseFromAnswer(200, { "content-type": "image/png" }, png);
  assert.equal(response.headers.get("Content-Type"), "image/png");
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), png);
});

test("json answers stay text", async () => {
  const response = responseFromAnswer(200, { "content-type": "application/json" }, new TextEncoder().encode('{"ok":true}'));
  assert.deepEqual(await response.json(), { ok: true });
});

test("a blob body is sent as its bytes", async () => {
  const bytes = Uint8Array.from([0xff, 0xd8, 0xff, 0x01]);
  const blob = new Blob([bytes], { type: "image/jpeg" });
  assert.deepEqual(await bodyBytes(blob), bytes);
});
