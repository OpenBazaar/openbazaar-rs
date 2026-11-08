import assert from "node:assert/strict";
import { test } from "node:test";

import { couponDraft } from "./couponDraft.ts";

const form = {
  code: "fall10",
  kind: "percent" as const,
  amount: "1000",
  currency: "USD",
  minSubtotal: "",
  startsOn: "2026-11-01",
  endsOn: "2026-11-30",
  maxUses: "",
  listings: [] as string[],
};

test("the coupon the editor saves is the signed coupon, and an invalid coupon is refused", () => {
  const saved = couponDraft(form);
  assert.equal(typeof saved, "object");
  if (typeof saved === "string") return;
  assert.equal(saved.code, "FALL10");
  assert.equal(saved.percentBps, 1000);
  assert.equal(saved.fixed, undefined);
  assert.equal(saved.startsOn, 20261101);
  assert.equal(saved.endsOn, 20261130);
  assert.equal(couponDraft({ ...form, code: "" }), "A coupon code is 3 to 24 uppercase letters, digits or dashes.");
  assert.equal(couponDraft({ ...form, kind: "percent", amount: "0" }), "A coupon takes 0.01% to 100% off.");
  assert.equal(couponDraft({ ...form, kind: "fixed", amount: "0" }), "A coupon takes more than zero off.");
  assert.equal(couponDraft({ ...form, startsOn: "2026-12-01", endsOn: "2026-11-01" }), "A coupon ends before it starts.");
});
