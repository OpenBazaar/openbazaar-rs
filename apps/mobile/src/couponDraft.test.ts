import assert from "node:assert/strict";
import { test } from "node:test";

import { couponDraft, couponEditorDraft } from "./couponDraft.ts";

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

test("merchant percentages and currency amounts reach the node in exact integer units", () => {
  const percent = couponEditorDraft({ ...form, amount: "12.50", minSubtotal: "49.95" });
  assert.equal(typeof percent, "object");
  if (typeof percent !== "string") {
    assert.equal(percent.percentBps, 1250);
    assert.equal(percent.minSubtotal, 4995);
  }
  const fixed = couponEditorDraft({ ...form, kind: "fixed", amount: "5.25", currency: "EUR" });
  if (typeof fixed === "string") assert.fail(fixed);
  assert.equal(fixed.fixed, 525);
  assert.equal(fixed.currency, "EUR");
  const yen = couponEditorDraft({ ...form, kind: "fixed", amount: "500", currency: "JPY", minSubtotal: "2000" });
  if (typeof yen === "string") assert.fail(yen);
  assert.equal(yen.fixed, 500);
  assert.equal(yen.minSubtotal, 2000);
  assert.equal(typeof couponEditorDraft({ ...form, amount: "100.01" }), "string");
  assert.equal(typeof couponEditorDraft({ ...form, amount: "0" }), "string");
  assert.equal(typeof couponEditorDraft({ ...form, amount: "1.001" }), "string");
  assert.equal(typeof couponEditorDraft({ ...form, amount: "10", minSubtotal: "9007199254740993" }), "string");
});
