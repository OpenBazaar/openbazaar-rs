import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { decodeListingTerms, fromHex, verifyCoupon, type Commerce } from "./commerce.ts";
import { quote, type Delivery } from "./pricing.ts";
import { choose, initialSelection, maxQuantity, optionStates, selectedVariant, stockNote } from "./variants.ts";

type Vector = {
  name: string;
  today: number;
  records: string[];
  lines: { listing: number; variant: string; quantity: number; delivery: Delivery }[];
  coupon: string | null;
  expect: { error: string } | { currency: string; units: number[]; subtotals: number[]; items: number; couponOff: number; shipping: number; total: number };
};

const vectors = (): Vector[] =>
  (JSON.parse(readFileSync(new URL("../../../crates/protocol/vectors/pricing.json", import.meta.url), "utf8")) as { vectors: Vector[] }).vectors;

test("the client prices every golden vector exactly as the Rust rules do", () => {
  const all = vectors();
  assert.ok(all.length >= 10);
  for (const v of all) {
    const listings = v.records.map(hex => decodeListingTerms(fromHex(hex)));
    const coupon = v.coupon ? verifyCoupon(v.coupon) : null;
    if (v.coupon) assert.ok(coupon, `${v.name}: coupon verifies`);
    const result = quote(v.lines.map(l => ({ listing: listings[l.listing], variant: l.variant, quantity: l.quantity, delivery: l.delivery })), coupon, { today: v.today, now: 1_790_000_000 });
    if ("error" in v.expect) {
      assert.equal(result.ok ? "ok" : result.error, v.expect.error, v.name);
    } else {
      assert.ok(result.ok, `${v.name}: ${result.ok ? "" : result.error}`);
      const { currency, units, subtotals, items, couponOff, shipping, total } = result.quote;
      assert.deepEqual({ currency, units, subtotals, items, couponOff, shipping, total }, v.expect, v.name);
    }
  }
});

test("v7 records decode with variants, shipping and terms", () => {
  const tee = decodeListingTerms(fromHex(vectors()[0].records[0]));
  assert.equal(tee.version, 7);
  assert.equal(tee.price?.currency, "USD");
  const commerce = tee.commerce!;
  assert.deepEqual(commerce.axes.map(a => a.name), ["Size", "Color"]);
  assert.equal(commerce.variants.length, 4);
  assert.equal(tee.stock, commerce.variants.reduce((n, v) => n + v.stock, 0));
  assert.equal(commerce.shipping.pickup, "Oregon");
  assert.deepEqual(commerce.returns, { kind: "days", days: 30, sellerPays: false });
});

test("a tampered coupon does not verify", () => {
  const hex = vectors().find(v => v.coupon)!.coupon!;
  assert.ok(verifyCoupon(hex));
  const bytes = fromHex(hex);
  bytes[40] ^= 1;
  assert.equal(verifyCoupon(Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")), null);
});

function tee(): Commerce {
  return decodeListingTerms(fromHex(vectors()[0].records[0])).commerce!;
}

test("the picker greys out combinations that are missing or sold out", () => {
  const commerce = tee();
  // Variants: S/Navy (4), M/Navy (0), L/Navy (2), M/Sand (6).
  const start = initialSelection(commerce);
  assert.deepEqual(start, [0, 0]);
  assert.deepEqual(optionStates(commerce, start), [["available", "sold-out", "available"], ["available", "unavailable"]]);
  const medium = choose(commerce, start, 0, 1);
  assert.deepEqual(medium, [1, 0]);
  assert.equal(selectedVariant(commerce, medium)?.stock, 0);
  assert.deepEqual(optionStates(commerce, medium)[1], ["sold-out", "available"]);
  const sand = choose(commerce, [0, 0], 1, 1);
  assert.deepEqual(sand, [null, 1], "small sand is not offered, so the size is cleared");
  assert.deepEqual(optionStates(commerce, sand)[0], ["unavailable", "available", "unavailable"]);
});

test("stock is worded as of the signing day and capped per order", () => {
  const commerce = tee();
  assert.deepEqual(stockNote(2, commerce.availability, 20261107), { tone: "low", text: "Only 2 left as of Nov 7" });
  assert.equal(stockNote(0, commerce.availability, 20261107).text, "Sold out");
  assert.equal(stockNote(Infinity, { kind: "made-to-order", days: 5 }, 20261107).text, "Made to order · ships in 5 days");
  assert.equal(maxQuantity(commerce, 12), 5);
  assert.equal(maxQuantity(commerce, 2), 2);
});
