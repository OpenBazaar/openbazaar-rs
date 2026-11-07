import assert from "node:assert/strict";
import { test } from "node:test";
import { x25519 } from "@noble/curves/ed25519.js";
import { p256 } from "@noble/curves/nist.js";
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { LAN_HOST } from "./client.ts";
import { encodeRequest } from "./http1.ts";
import { lanSeal } from "./lan.ts";
import { base32, deviceIdOf, discoveryKeyText, nodeIdOf, pairingOnion, pairProof, parsePairingLink, requestPreimage, sas, transcript } from "./protocol.ts";
import vectors from "../../../../crates/node/src/remote/vectors.json" with { type: "json" };
import { secretToWords, wordsToSecret } from "./words.ts";

// The node's outputs for fixed inputs (crates/node/src/remote/vectors.json).

const v = vectors;
const pairSecret = hexToBytes(v.pairing.secret);
const nodePublic = hexToBytes(v.node.public);

test("node id and device ids", () => {
  assert.equal(bytesToHex(x25519.getPublicKey(hexToBytes(v.node.secret))), v.node.public);
  assert.equal(nodeIdOf(nodePublic), v.node.nodeId);
  assert.equal(`p256:${bytesToHex(p256.getPublicKey(hexToBytes(v.keys.p256.secret), true))}`, v.keys.p256.text);
  assert.equal(`bip340:${bytesToHex(schnorr.getPublicKey(hexToBytes(v.keys.bip340.secret)))}`, v.keys.bip340.text);
  for (const key of [v.keys.p256, v.keys.watch, v.keys.bip340]) assert.equal(deviceIdOf(key.text), key.deviceId);
});

test("discovery key: the same key, but the spec's lowercase base32", () => {
  const secret = hexToBytes(v.discovery.secret);
  assert.equal(base32(secret), v.discovery.secretBase32);
  const ours = discoveryKeyText(x25519.getPublicKey(secret));
  assert.equal(ours.toUpperCase(), v.discovery.public.toUpperCase());
  assert.equal(ours, ours.toLowerCase());
});

test("pairing secret: base32, words, typed prefixes, link, onion", () => {
  assert.equal(base32(pairSecret), v.pairing.secretBase32);
  assert.deepEqual(secretToWords(pairSecret), v.pairing.words);
  assert.deepEqual(wordsToSecret(v.pairing.typed), { secret: pairSecret });
  assert.equal(`${pairingOnion(pairSecret)}.onion`, v.pairing.onion);
  const link = parsePairingLink(v.pairing.link);
  assert.ok(link);
  assert.deepEqual(link.secret, pairSecret);
  assert.deepEqual(link.nodePublic, nodePublic);
  assert.equal(link.onion, new URL(v.pairing.link.replace("openbazaar:", "https:")).searchParams.get("o"));
  assert.deepEqual(link.lan, ["192.168.1.20:8790"]);
});

test("pair request: transcript, proof, signature, SAS", () => {
  const text = transcript(v.request.fields);
  assert.equal(text, v.request.transcript);
  assert.equal(pairProof(pairSecret, text), v.request.proof);
  assert.ok(p256.verify(hexToBytes(v.request.signature), utf8ToBytes(text), hexToBytes(v.keys.p256.text.slice(5)), { format: "der" }));
  const pictures = sas(pairSecret, nodePublic, text);
  assert.deepEqual(pictures.map(item => item.emoji), v.request.sas);
  assert.deepEqual(pictures.map(item => item.name), v.request.sasNames);

  const bare = transcript({ watchKey: "", discoveryKey: "", ...v.request.bare.fields });
  assert.equal(bare, v.request.bare.transcript);
  assert.equal(pairProof(pairSecret, bare), v.request.bare.proof);
});

test("signed request preimage and both signature kinds", () => {
  const r = v.signedRequest;
  const body = utf8ToBytes(r.body);
  const p256Preimage = requestPreimage(v.node.nodeId, v.keys.p256.deviceId, r.time, r.nonce, r.method, r.path, body);
  assert.equal(p256Preimage, r.p256.preimage);
  assert.ok(p256.verify(hexToBytes(r.p256.signature), utf8ToBytes(p256Preimage), hexToBytes(v.keys.p256.text.slice(5)), { format: "der" }));
  const bipPreimage = requestPreimage(v.node.nodeId, v.keys.bip340.deviceId, r.time, r.nonce, r.method, r.path, body);
  assert.equal(bipPreimage, r.bip340.preimage);
  assert.ok(schnorr.verify(hexToBytes(r.bip340.signature), sha256(utf8ToBytes(bipPreimage)), hexToBytes(v.keys.bip340.text.slice(7))));
});

test("LAN channel: sealed request and answer", () => {
  const ephemeral = hexToBytes(v.lan.ephemeralSecret);
  const inner = encodeRequest("GET", "/remote/hello", LAN_HOST, {}, new Uint8Array());
  assert.ok(new TextDecoder().decode(inner).startsWith(v.lan.request.split("\r\n").slice(0, 2).join("\r\n") + "\r\n"));
  const sealed = lanSeal(nodePublic, utf8ToBytes(v.lan.request), ephemeral);
  assert.equal(bytesToHex(sealed.body), v.lan.body);
  assert.equal(new TextDecoder().decode(sealed.open(hexToBytes(v.lan.sealedAnswer))), v.lan.answer);
});
