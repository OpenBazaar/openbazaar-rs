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
  });

  assert.deepEqual(parseReceiptHash(hash, "http://127.0.0.1:8080"), {
    phrase: VALID_PHRASE,
    stores: [VENDOR_ONION, "http://127.0.0.1:8080"],
  });
});

test("buildReceiptRestoreUrl throws on invalid phrase or origin", () => {
  assert.throws(() => buildReceiptRestoreUrl(MODERATOR_ONION, "invalid phrase"));
  assert.throws(() => buildReceiptRestoreUrl("not-a-url", VALID_PHRASE));
});
