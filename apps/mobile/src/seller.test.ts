import assert from "node:assert/strict";
import { test } from "node:test";

import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { parseBuyerOrder } from "./catalog.ts";
import { bytesToHex, verifyListingRecord } from "./records.ts";
import { displayNameError, draftOf, formatPrice, fromMinorUnits, parseFollows, parseOwnListings, parseProfile, parseSale, salesToShip, toMinorUnits } from "./seller.ts";

const SECRET = new Uint8Array(32).fill(7);
const STORE = bytesToHex(schnorr.getPublicKey(SECRET));
const ID = "0123456789abcdef0123456789abcdef";
const IMAGE = "ab".repeat(32);

/// A version 6 listing laid out as crates/protocol writes it, signed with SECRET.
function v6Listing(price: { currency: string; amount: number; shipping: number } | null): string {
  const out: number[] = [];
  const enc = new TextEncoder();
  const u = (value: number, bytes: number) => {
    for (let i = 0; i < bytes; i++) out.push(Math.floor(value / 256 ** i) % 256);
  };
  const str = (text: string) => {
    const b = enc.encode(text);
    u(b.length, 2);
    out.push(...b);
  };
  const raw = (hex: string) => out.push(...Array.from({ length: hex.length / 2 }, (_, i) => parseInt(hex.slice(i * 2, i * 2 + 2), 16)));
  out.push(...enc.encode("OBLS"), 6);
  raw(STORE);
  str("Stoneware mug");
  str("Home");
  u(25000, 8);
  str("US");
  str("Wheel thrown.");
  str("Brook and Pine");
  u(5, 4);
  u(20260107, 4);
  out.push(0, 0);
  str("");
  u(3000, 8);
  u(0, 8);
  out.push(1);
  u(0b10, 2);
  raw(ID);
  out.push(1);
  raw(IMAGE);
  if (price) {
    out.push(1);
    str(price.currency);
    u(price.amount, 8);
    u(price.shipping, 8);
  } else out.push(0);
  const body = Uint8Array.from(out);
  const tag = sha256(enc.encode("ob/listing/v1"));
  const digest = sha256(Uint8Array.from([...tag, ...tag, ...body]));
  return bytesToHex(Uint8Array.from([...body, ...schnorr.sign(digest, SECRET)]));
}

test("a version 6 listing verifies with its id, images, and fiat price", () => {
  const record = verifyListingRecord(v6Listing({ currency: "USD", amount: 1250, shipping: 300 }));
  assert.ok(record);
  assert.equal(record.version, 6);
  assert.equal(record.listingId, ID);
  assert.deepEqual(record.images, [IMAGE]);
  assert.deepEqual(record.price, { currency: "USD", amount: 1250, shipping: 300 });
  assert.equal(record.contentRating, "sensitive");
  assert.deepEqual(record.contentTags, ["violence"]);
  const sats = verifyListingRecord(v6Listing(null));
  assert.equal(sats?.price, null);
});

test("a changed version 6 byte fails", () => {
  const hex = v6Listing(null);
  const flipped = hex.slice(0, hex.length - 140) + (hex[hex.length - 140] === "0" ? "1" : "0") + hex.slice(hex.length - 139);
  assert.equal(verifyListingRecord(flipped), null);
});

test("own listings keep only records the own store signed, and round-trip to a draft", () => {
  const record = v6Listing({ currency: "EUR", amount: 900, shipping: 0 });
  const hash = verifyListingRecord(record)!.hash;
  const listings = parseOwnListings({ listings: [{ hash, record }, { hash: "00", record: "00" }] }, STORE);
  assert.equal(listings.length, 1);
  const draft = draftOf(listings[0]);
  assert.equal(draft.currency, "EUR");
  assert.equal(draft.price, 900);
  assert.equal(draft.contentRating, "sensitive");
  assert.deepEqual(parseOwnListings({ listings: [{ hash, record }] }, "ff".repeat(32)), []);
});

test("typed prices become minor units and back", () => {
  assert.equal(toMinorUnits("12.5", "USD"), 1250);
  assert.equal(toMinorUnits("12.50", "GBP"), 1250);
  assert.equal(toMinorUnits("1,200", "JPY"), 1200);
  assert.equal(toMinorUnits("25000", "SAT"), 25000);
  for (const bad of ["", "-1", "1.234", "abc", "1e3"]) assert.equal(toMinorUnits(bad, "USD"), null, bad);
  assert.equal(toMinorUnits("1.5", "JPY"), null);
  assert.equal(fromMinorUnits(1250, "USD"), "12.50");
  assert.equal(fromMinorUnits(1200, "JPY"), "1200");
  assert.equal(formatPrice(1250, "USD"), "$12.50");
  assert.equal(formatPrice(25000, "SAT"), "25,000 sats");
});

test("a store display name is required and at most 64 bytes", () => {
  assert.equal(displayNameError("  "), "Give the store a name.");
  assert.equal(displayNameError("Brook and Pine"), null);
  assert.equal(displayNameError("N".repeat(64)), null);
  assert.match(displayNameError("N".repeat(65)) ?? "", /64/);
  assert.equal(displayNameError("字".repeat(21)), null);
  assert.match(displayNameError("字".repeat(22)) ?? "", /64/);
});

test("the profile reads the node's values and drops unknown ones", () => {
  const profile = parseProfile({ role: "seller", storeKey: STORE, name: "Brook and Pine", avatar: IMAGE, banner: "not a hash", shipsTo: ["US", 4], xmrAvailable: false, published: true });
  assert.equal(profile.role, "seller");
  assert.equal(profile.avatar, IMAGE);
  assert.equal(profile.banner, null);
  assert.equal(profile.acceptsLightning, false);
  assert.equal(profile.away, false);
  const paused = parseProfile({ role: "seller", storeKey: STORE, away: true, vacationNote: "in Paris until June" });
  assert.equal(paused.away, true);
  assert.equal("vacationNote" in paused, false);
  assert.equal(parseProfile({ role: "seller", storeKey: STORE, acceptsLightning: true }).acceptsLightning, true);
  assert.deepEqual(profile.shipsTo, ["US"]);
  assert.equal(parseProfile({ role: "admin", storeKey: STORE }).role, null);
  assert.throws(() => parseProfile({ role: "seller" }));
});

test("a sale parses with its fulfillment, and paid sales are the ones to ship", () => {
  const sale = parseSale({ id: "ab12", status: "Paid", totalSats: 28000, lines: [{ title: "Mug", kind: "physical", quantity: 1, priceSats: 25000 }], txids: ["c".repeat(64), "bad"], fulfillment: null });
  assert.equal(sale.lines[0].title, "Mug");
  assert.deepEqual(sale.txids, ["c".repeat(64)]);
  const shipped = parseSale({ id: "cd34", status: "Shipped", fulfillment: { carrier: "USPS", tracking: "9400" } });
  assert.equal(shipped.fulfillment?.carrier, "USPS");
  assert.equal(salesToShip([sale, shipped]), 1);
  assert.throws(() => parseSale({ id: "../x" }));
});

test("follows parse, and a remote buyer order without payment terms yet reads", () => {
  assert.deepEqual(parseFollows({ follows: [{ url: "http://127.0.0.1:9", storeId: null, name: null, listings: 0, fetched: null, error: "offline" }] }), [
    { url: "http://127.0.0.1:9", storeId: null, name: null, listings: 0, fetched: null, error: "offline" },
  ]);
  const order = parseBuyerOrder({
    id: "o1",
    created: 1,
    seller: "Brook and Pine",
    storeId: STORE,
    totalSats: 28000,
    settlement: "direct",
    status: "Awaiting seller",
    remote: { delivered: true, fulfillment: { carrier: "USPS", tracking: "9400", note: "", content: "" } },
  });
  assert.equal(order.payment, null);
  assert.equal(order.remote?.delivered, true);
  assert.equal(order.remote?.fulfillment?.tracking, "9400");
});
