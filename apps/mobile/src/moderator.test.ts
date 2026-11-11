import assert from "node:assert/strict";
import { test } from "node:test";

import { disputeState, parseDisputes, partyName } from "./moderator.ts";

const letter = (favors: string, ruled = false) => ({ id: "ab".repeat(16), favors, memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled });

test("disputes parse as the node sends them", () => {
  const [dispute] = parseDisputes({ disputes: [letter("buyer")] });
  assert.deepEqual(dispute, { id: "ab".repeat(16), favors: "buyer", memo: "No parcel arrived.", outpoint: `${"cd".repeat(32)}:0`, receivedHeight: 160400, ruled: false, builtIn: false });
  assert.equal(parseDisputes({ disputes: [{ ...letter("buyer"), builtIn: true }] })[0].builtIn, true);
  assert.deepEqual(parseDisputes({ disputes: [] }), []);
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
