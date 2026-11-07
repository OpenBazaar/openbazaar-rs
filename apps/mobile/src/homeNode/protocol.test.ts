import assert from "node:assert/strict";
import { test } from "node:test";
import { p256 } from "@noble/curves/nist.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, concatBytes, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";

import { decodeResponse, encodeRequest } from "./http1.ts";
import { lanSeal } from "./lan.ts";
import {
  base32,
  deviceIdOf,
  discoveryKeyText,
  fromBase32,
  nodeIdOf,
  onionFromPublicKey,
  pairingOnion,
  pairProof,
  parsePairingLink,
  requestPreimage,
  SAS_EMOJI,
  sas,
  transcript,
} from "./protocol.ts";
import { secretToWords, wordIndex, wordsToSecret } from "./words.ts";

const secret = hexToBytes("00112233445566778899");
const nodePublic = x25519.getPublicKey(new Uint8Array(32).fill(9));

test("the SAS list is the spec's 64 pictures", () => {
  assert.equal(SAS_EMOJI.length, 64);
  assert.deepEqual(SAS_EMOJI[0], { emoji: "🐶", name: "dog" });
  assert.deepEqual(SAS_EMOJI[30], { emoji: "☀️", name: "sun" });
  assert.deepEqual(SAS_EMOJI[63], { emoji: "👑", name: "crown" });
  assert.equal(new Set(SAS_EMOJI.map(item => item.name)).size, 64);
});

test("pairing words carry the secret and a checksum", () => {
  const words = secretToWords(secret);
  assert.equal(words.length, 8);
  assert.deepEqual(wordsToSecret(words.join(" ")), { secret });
  const typed = words.map(word => word.slice(0, 4).toUpperCase()).join("  ");
  assert.deepEqual(wordsToSecret(typed), { secret });
  const swapped = [...words];
  [swapped[0], swapped[1]] = [swapped[1], swapped[0]];
  assert.ok("error" in wordsToSecret(swapped.join(" ")));
  assert.ok("error" in wordsToSecret(words.slice(1).join(" ")));
  const bad = wordsToSecret(["zzzz", ...words.slice(1)].join(" "));
  assert.ok("badWord" in bad && bad.badWord === 0);
  assert.equal(wordIndex("abandon"), 0);
  assert.equal(wordIndex("zoo"), 2047);
  assert.equal(wordIndex("zo"), -1);
});

test("the first word holds the secret's top 11 bits", () => {
  const words = secretToWords(new Uint8Array(10).fill(0xff));
  assert.equal(words[0], "zoo");
  const zero = secretToWords(new Uint8Array(10));
  assert.equal(zero[0], "abandon");
  assert.equal(wordIndex(zero[7]), sha256(new Uint8Array(10))[0]);
});

test("onion addresses follow rend-spec-v3", () => {
  const known = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";
  assert.equal(onionFromPublicKey(fromBase32(known).slice(0, 32)), known);
  const onion = pairingOnion(secret);
  assert.match(onion, /^[a-z2-7]{55}d$/);
  assert.equal(pairingOnion(secret), onion);
  assert.notEqual(pairingOnion(new Uint8Array(10)), onion);
});

test("ids, transcript, proof, and SAS", () => {
  assert.equal(nodeIdOf(nodePublic), bytesToHex(sha256(concatBytes(utf8ToBytes("OBNODE2"), nodePublic))).slice(0, 32));
  const key = `p256:${bytesToHex(p256.getPublicKey(new Uint8Array(32).fill(1), true))}`;
  assert.equal(deviceIdOf(key).length, 16);
  const fields = { key, discoveryKey: discoveryKeyText(new Uint8Array(32)), name: "Brian's iPhone", platform: "ios" as const };
  const text = transcript(fields);
  assert.equal(text, `OBPAIR2\n${key}\n\n${fields.discoveryKey}\nBrian's iPhone\nios`);
  assert.equal(fields.discoveryKey, `descriptor:x25519:${"a".repeat(52)}`);
  assert.match(pairProof(secret, text), /^[0-9a-f]{64}$/);
  const h = sha256(concatBytes(utf8ToBytes("OBSAS2\n"), secret, nodePublic, utf8ToBytes(text)));
  assert.deepEqual(sas(secret, nodePublic, text), [0, 1, 2, 3].map(i => SAS_EMOJI[h[i] & 63]));
});

test("request preimage and a P-256 signature the node can check", () => {
  const preimage = requestPreimage("00112233445566778899aabbccddeeff", "0011223344556677", 1700000000, "ab".repeat(16), "POST", "/listings?x=1", utf8ToBytes('{"a":1}'));
  assert.equal(preimage, `OBRQ2\n00112233445566778899aabbccddeeff\n0011223344556677\n1700000000\n${"ab".repeat(16)}\nPOST\n/listings?x=1\n${bytesToHex(sha256(utf8ToBytes('{"a":1}')))}`);
  const sk = new Uint8Array(32).fill(1);
  const der = p256.sign(utf8ToBytes(preimage), sk, { format: "der" });
  assert.ok(p256.verify(der, utf8ToBytes(preimage), p256.getPublicKey(sk, true), { format: "der" }));
});

test("pairing links parse only v2 with a 10-byte secret", () => {
  const onion = "a".repeat(56);
  const k = bytesToHex(nodePublic);
  const link = `openbazaar://pair?v=2&s=${base32(secret)}&k=${k}&o=${onion}&l=192.168.1.20:8790&l=10.0.0.5:8790`;
  assert.deepEqual(parsePairingLink(link), { secret, nodePublic, onion, lan: ["192.168.1.20:8790", "10.0.0.5:8790"] });
  assert.equal(parsePairingLink(link.replace("v=2", "v=1")), null);
  assert.equal(parsePairingLink(link.replace(base32(secret), base32(new Uint8Array(11)))), null);
  assert.equal(parsePairingLink(`openbazaar://pair?v=2&s=${base32(secret)}&k=${k}&o=short`), null);
});

test("the LAN channel seals for the node key only", () => {
  const nodeSecret = new Uint8Array(32).fill(9);
  const ephemeral = new Uint8Array(32).fill(5);
  const inner = encodeRequest("GET", "/catalog", "x.onion", {}, new Uint8Array());
  const sealed = lanSeal(nodePublic, inner, ephemeral);
  const E = sealed.body.slice(0, 32);
  const K = hkdf(sha256, x25519.getSharedSecret(nodeSecret, E), utf8ToBytes("OBLAN1"), concatBytes(E, nodePublic), 64);
  const aad = utf8ToBytes("OBLAN1");
  assert.deepEqual(chacha20poly1305(K.slice(0, 32), new Uint8Array(12), aad).decrypt(sealed.body.slice(32)), inner);
  const answer = utf8ToBytes("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
  const reply = chacha20poly1305(K.slice(32), new Uint8Array(12), aad).encrypt(answer);
  assert.deepEqual(sealed.open(reply), answer);
  assert.throws(() => sealed.open(chacha20poly1305(K.slice(0, 32), new Uint8Array(12), aad).encrypt(answer)));
});

test("HTTP/1.1 framing", () => {
  const request = new TextDecoder().decode(encodeRequest("POST", "/remote/sync", "n.onion", { "X-Ob-Time": "1" }, utf8ToBytes("{}")));
  assert.equal(request, "POST /remote/sync HTTP/1.1\r\nHost: n.onion\r\nConnection: close\r\nContent-Length: 2\r\nX-Ob-Time: 1\r\n\r\n{}");
  assert.throws(() => encodeRequest("GET", "/", "h", { a: "b\r\nc" }, new Uint8Array()));
  const plain = decodeResponse(utf8ToBytes("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 4\r\n\r\n{}xx"));
  assert.equal(plain.status, 401);
  assert.equal(new TextDecoder().decode(plain.body), "{}xx");
  const chunked = decodeResponse(utf8ToBytes("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n"));
  assert.equal(new TextDecoder().decode(chunked.body), "abcde");
  assert.throws(() => decodeResponse(utf8ToBytes("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort")));
});
