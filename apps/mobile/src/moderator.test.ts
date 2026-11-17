import assert from "node:assert/strict";
import { test } from "node:test";

import { disputeState, parseDisputes, parseDisputeTracking, partyName } from "./moderator.ts";

const letter = (favors: string, ruled = false) => ({ id: "ab".repeat(16), favors, memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled });

test("disputes parse as the node sends them", () => {
  const [dispute] = parseDisputes({ disputes: [letter("buyer")] });
  assert.deepEqual(dispute, { id: "ab".repeat(16), favors: "buyer", memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled: false, builtIn: false, tracking: null });
  assert.equal(parseDisputes({ disputes: [{ ...letter("buyer"), builtIn: true }] })[0].builtIn, true);
  assert.deepEqual(parseDisputes({ disputes: [] }), []);
});

const lookedUp = () => ({
  provider: "aftership",
  carrier: "usps",
  trackingNumber: "9400100000",
  status: "delivered",
  label: "Delivered",
  deliveredAt: "2026-11-13T15:04:05Z",
  updatedAt: 99,
  events: [{ at: "2026-11-13T15:04:05Z", status: "delivered", label: "Delivered", city: "Portland", state: "OR", country: "United States" }],
  shipTo: { city: "Portland", state: "Oregon", country: "" },
  locationCheck: { verdict: "match", reason: "The delivered checkpoint names the expected locality.", deliveredCity: "Portland", deliveredState: "OR", deliveredCountry: "United States" },
});

test("a dispute shipment lookup parses with its verdict", () => {
  const tracking = parseDisputeTracking(lookedUp());
  assert.equal(tracking?.status, "delivered");
  assert.equal(tracking?.trackingNumber, "9400100000");
  assert.equal(tracking?.shipTo?.city, "Portland");
  assert.equal(tracking?.locationCheck?.verdict, "match");
  const [dispute] = parseDisputes({ disputes: [{ ...letter("buyer"), tracking: lookedUp() }] });
  assert.equal(dispute.tracking?.locationCheck?.verdict, "match");
});

test("an invented verdict or lookup shape is dropped, not shown", () => {
  assert.equal(parseDisputeTracking(null), null);
  assert.equal(parseDisputeTracking("delivered"), null);
  const invented = { ...lookedUp(), locationCheck: { verdict: "trustworthy", reason: "just believe it" } };
  assert.equal(parseDisputeTracking(invented)?.locationCheck, null);
  const noCheck = parseDisputeTracking({ ...lookedUp(), shipTo: null, locationCheck: null });
  assert.equal(noCheck?.locationCheck, null);
  assert.equal(noCheck?.shipTo, null);
});

test("an unreadable desk is an error, not an empty desk", () => {
  assert.throws(() => parseDisputes({}));
  assert.throws(() => parseDisputes({ disputes: [letter("moderator")] }));
  assert.throws(() => parseDisputes({ disputes: [{ favors: "buyer" }] }));
});

test("one ruling closes both letters about an escrow", () => {
  const all = parseDisputes({ disputes: [letter("buyer"), letter("vendor", true)] });
  assert.equal(disputeState(all[0], all), "lost");
  assert.equal(disputeState(all[1], all), "granted");
  const open = parseDisputes({ disputes: [letter("buyer"), letter("vendor")] });
  assert.deepEqual(open.map(dispute => disputeState(dispute, open)), ["open", "open"]);
});

test("the vendor is the seller in the app's words", () => {
  assert.equal(partyName("vendor"), "seller");
  assert.equal(partyName("buyer"), "buyer");
});
