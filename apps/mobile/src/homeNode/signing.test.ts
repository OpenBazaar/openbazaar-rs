import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { accessKey, deviceKey, parseAccessKey, requestPreimage, sign, signedHeaders } from "./signing.ts";

// The node's own protocol v2 vectors (crates/node/src/remote/vectors.rs).
const vectors = JSON.parse(readFileSync(new URL("../../../../crates/node/src/remote/vectors.json", import.meta.url), "utf8"));
const bip340 = vectors.keys.bip340;
const request = vectors.signedRequest;
const nodeId: string = vectors.node.nodeId;
const verify = (signature: string, preimage: string, publicKey: string) =>
  schnorr.verify(hexToBytes(signature), sha256(utf8ToBytes(preimage)), hexToBytes(publicKey));

test("device keys and preimages match the node's vectors", () => {
  const key = deviceKey(hexToBytes(bip340.secret));
  assert.equal(`bip340:${key.publicKey}`, bip340.text);
  assert.equal(key.id, bip340.deviceId);
  const preimage = requestPreimage(nodeId, key.id, request.time, request.nonce, request.method, request.path, utf8ToBytes(request.body));
  assert.equal(preimage, request.bip340.preimage);
  assert.ok(verify(request.bip340.signature, preimage, key.publicKey), "the node's signature checks out here");
  assert.ok(verify(sign(key, preimage), preimage, key.publicKey));
});

test("signed headers carry a fresh nonce and a signature over the full target", () => {
  const key = deviceKey(hexToBytes(bip340.secret));
  const a = signedHeaders(key, nodeId, 1700000000, "GET", "/orders?status=open", new Uint8Array());
  const b = signedHeaders(key, nodeId, 1700000000, "GET", "/orders?status=open", new Uint8Array());
  assert.equal(a["X-Ob-Device"], key.id);
  assert.match(a["X-Ob-Nonce"], /^[0-9a-f]{32}$/);
  assert.notEqual(a["X-Ob-Nonce"], b["X-Ob-Nonce"]);
  const signed = requestPreimage(nodeId, key.id, 1700000000, a["X-Ob-Nonce"], "GET", "/orders?status=open", new Uint8Array());
  assert.ok(verify(a["X-Ob-Signature"], signed, key.publicKey));
  const withoutQuery = requestPreimage(nodeId, key.id, 1700000000, a["X-Ob-Nonce"], "GET", "/orders", new Uint8Array());
  assert.ok(!verify(a["X-Ob-Signature"], withoutQuery, key.publicKey));
});

test("access keys round-trip and refuse typos", () => {
  const secret = new Uint8Array(32).fill(7);
  const text = accessKey(secret);
  assert.equal(text, "obkey-a4dqobyha4dqobyha4dqobyha4dqobyha4dqobyha4dqobyha4dq");
  assert.deepEqual(parseAccessKey(` ${text.toUpperCase().replace("OBKEY-", "obkey-")} `), secret);
  assert.equal(parseAccessKey(text.slice(0, -1)), null);
  assert.equal(parseAccessKey("obhome:ABC@x.onion"), null);
});
