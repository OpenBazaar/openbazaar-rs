import assert from "node:assert/strict";
import { test } from "node:test";

import { imageDataUri } from "./imageUri.ts";

test("a catalog image is a data URI, not a loopback or cleartext URL", () => {
  const uri = imageDataUri("image/jpeg", Uint8Array.from([0xff, 0xd8, 0xff]));
  assert.equal(uri, "data:image/jpeg;base64,/9j/");
  assert.equal(uri.includes("http"), false);
  assert.equal(uri.includes("127.0.0.1"), false);
});
