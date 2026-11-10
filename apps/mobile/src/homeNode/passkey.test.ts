import assert from "node:assert/strict";
import { test } from "node:test";
import { bytesToHex } from "@noble/hashes/utils.js";

import { boundChallenge, passkeyKeyText } from "./passkey.ts";

test("a passkey's public key is sent as the compressed P-256 point", async () => {
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const spki = new Uint8Array(await crypto.subtle.exportKey("spki", pair.publicKey));
  const point = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
  const compressed = new Uint8Array([2 + (point[64] & 1), ...point.slice(1, 33)]);
  assert.equal(passkeyKeyText(spki), `p256:${bytesToHex(compressed)}`);
  assert.throws(() => passkeyKeyText(new Uint8Array(44)), /not supported/);
});

test("the passkey signs the node's challenge with the session key (same vector as the node's test)", () => {
  const challenge = boundChallenge("07".repeat(32), `bip340:${"ab".repeat(32)}`);
  assert.equal(bytesToHex(challenge), "8bfc3cded9cbeff69929711ce725eb1f03506650d8e69dbf42c4e2ebca1a58f3");
});
