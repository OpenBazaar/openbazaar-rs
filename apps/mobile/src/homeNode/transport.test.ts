import assert from "node:assert/strict";
import { test } from "node:test";
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { deviceKey, requestPreimage } from "./signing.ts";
import { signedFetch, signIn } from "./transport.ts";

const nodeId = "90".repeat(32);
const secret = new Uint8Array(32).fill(5);

// `POST /media` carries the image as the raw body (src/seller.ts). The node
// checks the signature over the bytes it received.
test("an uploaded image is signed over its bytes and sent", async () => {
  const sent: { path: string; init: RequestInit }[] = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (async (path: string, init: RequestInit = {}) => {
    sent.push({ path, init });
    return Response.json(path === "/remote/hello" ? { nodeId } : {});
  }) as typeof fetch;
  try {
    await signIn(secret, false);
    const image = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 255, 128]);
    const file = new Blob([image], { type: "image/png" });
    const response = await signedFetch("/media", { method: "POST", headers: { "Content-Type": "image/png" }, body: file });
    assert.equal(response.status, 200);
    const { path, init } = sent[sent.length - 1];
    const headers = init.headers as Record<string, string>;
    assert.equal(path, "/media");
    assert.equal(init.body, file);
    assert.equal(headers["Content-Type"], "image/png");
    const key = deviceKey(secret);
    const preimage = requestPreimage(nodeId, key.id, Number(headers["X-Ob-Time"]), headers["X-Ob-Nonce"], "POST", "/media", image);
    assert.ok(schnorr.verify(hexToBytes(headers["X-Ob-Signature"]), sha256(utf8ToBytes(preimage)), hexToBytes(key.publicKey)));
  } finally {
    globalThis.fetch = realFetch;
  }
});
