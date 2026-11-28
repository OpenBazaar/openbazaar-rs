import assert from "node:assert/strict";
import { test } from "node:test";

import { buildReceiptRestoreUrl, parseReceiptHash } from "./torReceiptLink.ts";

const VALID_PHRASE =
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const MODERATOR_ONION = "http://moderatorexample1234567890abcdefghijklmnopqrstuvwxyz234567.onion";
const VENDOR_ONION = "http://vendorexample1234567890abcdefghijklmnopqrstuvwxyz23456789.onion";
const VENDOR_ONION_2 = "http://vendor2example1234567890abcdefghijklmnopqrstuvwxyz2345678.onion";

test("buildReceiptRestoreUrl and parseReceiptHash roundtrip with store URLs", () => {
  const url = buildReceiptRestoreUrl(
    `${MODERATOR_ONION}/`,
    `  ${VALID_PHRASE.toUpperCase()}  `,
    [`${VENDOR_ONION}/catalog`, VENDOR_ONION, VENDOR_ONION_2],
  );

  const parsedUrl = new URL(url);
  assert.equal(parsedUrl.origin, MODERATOR_ONION);
  assert.equal(parsedUrl.pathname, "/");
  assert.equal(parsedUrl.search, "", "phrase must NEVER appear in query string");
  assert.ok(parsedUrl.hash.startsWith("#receipt="), "hash must start with #receipt=");

  const parsed = parseReceiptHash(parsedUrl.hash, MODERATOR_ONION);
  assert.deepEqual(parsed, {
    phrase: VALID_PHRASE,
    stores: [VENDOR_ONION, VENDOR_ONION_2],
    cids: [],
  });
});

test("buildReceiptRestoreUrl works without vendorUrls and never uses query params", () => {
  const url = buildReceiptRestoreUrl(MODERATOR_ONION, VALID_PHRASE);
  const parsedUrl = new URL(url);
  assert.equal(parsedUrl.search, "");
  assert.ok(parsedUrl.hash.startsWith("#receipt="));

  const parsed = parseReceiptHash(parsedUrl.hash);
  assert.deepEqual(parsed, {
    phrase: VALID_PHRASE,
    stores: [],
    cids: [],
  });
});

test("parseReceiptHash rejects invalid phrases and non-receipt hashes", () => {
  assert.equal(parseReceiptHash(""), null);
  assert.equal(parseReceiptHash("#"), null);
  assert.equal(parseReceiptHash("#/"), null);
  assert.equal(parseReceiptHash("#/orders"), null);
  assert.equal(parseReceiptHash("#/product/deadbeef"), null);
  assert.equal(parseReceiptHash("#receipt=not+a+valid+twelve+word+phrase"), null);
  assert.equal(parseReceiptHash(`?receipt=${encodeURIComponent(VALID_PHRASE)}`), null);
});

test("parseReceiptHash filters untrusted store URLs and allows same-origin", () => {
  const hash =
    `#receipt=${encodeURIComponent(VALID_PHRASE)}` +
    `&store=${encodeURIComponent("https://evil.example.com")}` +
    `&store=${encodeURIComponent("javascript:alert(1)")}` +
    `&store=${encodeURIComponent("http://user:pass@" + VENDOR_ONION.slice(7))}` +
    `&store=${encodeURIComponent("http://.onion")}` +
    `&store=${encodeURIComponent(VENDOR_ONION)}` +
    `&store=${encodeURIComponent("http://127.0.0.1:8080/foo")}`;

  assert.deepEqual(parseReceiptHash(hash), {
    phrase: VALID_PHRASE,
    stores: [VENDOR_ONION],
    cids: [],
  });

  assert.deepEqual(parseReceiptHash(hash, "http://127.0.0.1:8080"), {
    phrase: VALID_PHRASE,
    stores: [VENDOR_ONION, "http://127.0.0.1:8080"],
    cids: [],
  });
});

test("buildReceiptRestoreUrl and parseReceiptHash roundtrip legacy cids, skip tor-N, and validate", () => {
  const legacyHex = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
  const legacyCustom = "chk_legacy-01";
  const url = buildReceiptRestoreUrl(
    MODERATOR_ONION,
    VALID_PHRASE,
    [VENDOR_ONION],
    [
      "tor-0",
      "tor-1",
      "tor-42",
      legacyHex,
      legacyHex, // duplicate
      ` ${legacyCustom} `,
      "bad cid with spaces",
      "../evil",
      "x".repeat(65), // too long
    ],
  );

  const parsedUrl = new URL(url);
  assert.equal(parsedUrl.search, "");
  assert.ok(parsedUrl.hash.includes(`cid=${legacyHex}`));
  assert.ok(!parsedUrl.hash.includes("tor-0"));
  assert.ok(!parsedUrl.hash.includes("tor-1"));

  assert.deepEqual(parseReceiptHash(parsedUrl.hash, MODERATOR_ONION), {
    phrase: VALID_PHRASE,
    stores: [VENDOR_ONION],
    cids: [legacyHex, legacyCustom],
  });

  // Direct hash parsing also rejects invalid cids, skips tor-N, dedupes, and caps at 20.
  const manyCids = Array.from({ length: 25 }, (_, i) => `legacy_${i}`);
  const rawHash =
    `#receipt=${encodeURIComponent(VALID_PHRASE)}` +
    `&cid=tor-0&cid=bad%20id&cid=${encodeURIComponent("a/b")}` +
    manyCids.map((id) => `&cid=${encodeURIComponent(id)}`).join("");
  const parsedMany = parseReceiptHash(rawHash);
  assert.ok(parsedMany);
  assert.equal(parsedMany.cids.length, 20);
  assert.deepEqual(parsedMany.cids, manyCids.slice(0, 20));
});

test("buildReceiptRestoreUrl throws on invalid phrase or origin", () => {
  assert.throws(() => buildReceiptRestoreUrl(MODERATOR_ONION, "invalid phrase"));
  assert.throws(() => buildReceiptRestoreUrl("not-a-url", VALID_PHRASE));
});
