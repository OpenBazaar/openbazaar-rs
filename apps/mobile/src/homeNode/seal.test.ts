import assert from "node:assert/strict";
import { test } from "node:test";

import { openJson, sealJson } from "./seal.ts";

test("sealed files open only under their key and refuse tampering", () => {
  const key = new Uint8Array(32).fill(1);
  const sealed = sealJson(key, { a: [1, "two"] });
  assert.deepEqual(openJson(key, sealed), { a: [1, "two"] });
  assert.notDeepEqual(sealJson(key, { a: 1 }), sealJson(key, { a: 1 }));
  assert.equal(openJson(new Uint8Array(32).fill(2), sealed), null);
  const tampered = sealed.slice();
  tampered[tampered.length - 1] ^= 1;
  assert.equal(openJson(key, tampered), null);
  assert.equal(openJson(key, new Uint8Array(10)), null);
});
