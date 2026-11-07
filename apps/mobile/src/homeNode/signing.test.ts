import assert from "node:assert/strict";
import { test } from "node:test";
import { schnorr } from "@noble/curves/secp256k1.js";
import { bytesToHex, hexToBytes } from "@noble/hashes/utils.js";

import { accessKey, deviceKey, pairDigest, parseAccessKey, parsePairingCode, requestDigest, sign, signedHeaders } from "./signing.ts";

// The same vector is checked in crates/node/src/remote/auth.rs.
const secret = new Uint8Array(32).fill(7);
const nodeId = "00112233445566778899aabbccddeeff";
const nonce = "0123456789abcdef0123456789abcdef";
const body = new TextEncoder().encode('{"a":1}');

test("device keys and digests match the node's", () => {
  const key = deviceKey(secret);
  assert.equal(key.publicKey, "989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f");
  assert.equal(key.id, "3f662275b53a056c");
  const digest = requestDigest(nodeId, key.id, 1700000000, nonce, "POST", "/listings?x=1", body);
  assert.equal(bytesToHex(digest), "5e8bcb08b73a1aa2f6d4d6988896ba02356f97ea7263da63045c5e39d22c1e77");
  assert.equal(bytesToHex(pairDigest("abcdefghijklmnopqrstuvwxyz234567", key.publicKey, "Phone")), "7a2dcd64d847362dfd278ef8eddab148d4385f18a012e60cdb3adab4ababa66c");
  assert.ok(schnorr.verify(hexToBytes(sign(key, digest)), digest, hexToBytes(key.publicKey)));
});

test("signed headers carry a fresh nonce and a signature over the request", () => {
  const key = deviceKey(secret);
  const a = signedHeaders(key, nodeId, 1700000000, "GET", "/catalog", new Uint8Array());
  const b = signedHeaders(key, nodeId, 1700000000, "GET", "/catalog", new Uint8Array());
  assert.equal(a["X-Ob-Device"], key.id);
  assert.match(a["X-Ob-Nonce"], /^[0-9a-f]{32}$/);
  assert.notEqual(a["X-Ob-Nonce"], b["X-Ob-Nonce"]);
  const digest = requestDigest(nodeId, key.id, 1700000000, a["X-Ob-Nonce"], "GET", "/catalog", new Uint8Array());
  assert.ok(schnorr.verify(hexToBytes(a["X-Ob-Signature"]), digest, hexToBytes(key.publicKey)));
});

test("access keys round-trip and refuse typos", () => {
  const text = accessKey(secret);
  assert.equal(text, "obkey-a4dqobyha4dqobyha4dqobyha4dqobyha4dqobyha4dqobyha4dq");
  assert.deepEqual(parseAccessKey(` ${text.toUpperCase().replace("OBKEY-", "obkey-")} `), secret);
  assert.equal(parseAccessKey(text.slice(0, -1)), null);
  assert.equal(parseAccessKey("obhome:ABC@x.onion"), null);
});

test("pairing codes parse only the node's format", () => {
  const onion = `${"a".repeat(56)}.onion`;
  assert.deepEqual(parsePairingCode(` obhome:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567@${onion} `), { secret: "abcdefghijklmnopqrstuvwxyz234567", address: onion });
  assert.equal(parsePairingCode("obhome:short@x.onion"), null);
  assert.equal(parsePairingCode(`obkey-${"a".repeat(52)}`), null);
});
