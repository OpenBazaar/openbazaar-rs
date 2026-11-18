import assert from "node:assert/strict";
import { test } from "node:test";

import { directoryFeeOf, directoryFeeStatus } from "./directoryFee.ts";

const address = "tb1p" + "q".repeat(58);

test("the fee keeps the address, amount, and a known state", () => {
  assert.deepEqual(directoryFeeOf({ available: true, address, sats: 20000, confirmations: 1, epochBlocks: 2016, state: "listed", listedUntilHeight: 120016, error: null }), {
    available: true, address, sats: 20000, confirmations: 1, state: "listed", listedUntilHeight: 120016,
  });
  const odd = directoryFeeOf({ available: true, address, sats: 20000, state: "paid?" });
  assert.equal(odd.available && odd.state, "checking");
});

test("an unavailable or unreadable answer gives a reason, never an address", () => {
  assert.deepEqual(directoryFeeOf({ available: false, reason: "The directory fee runs on testnet4 only." }), { available: false, reason: "The directory fee runs on testnet4 only." });
  for (const unreadable of [null, "fee", {}, { available: true, sats: 20000 }]) assert.equal(directoryFeeOf(unreadable).available, false);
});

test("the status says what the seller does next", () => {
  const fee = { available: true as const, address, sats: 20000, confirmations: 1, listedUntilHeight: 120016 };
  assert.equal(directoryFeeStatus({ ...fee, state: "unpaid" }), "Not listed yet. Your node could not pay from its bitcoin wallet. Send 20,000 sats to this address from any wallet, exchange, or swap.");
  assert.equal(directoryFeeStatus({ ...fee, state: "confirming" }), "Payment seen. Your store is listed after 1 confirmation.");
  assert.match(directoryFeeStatus({ ...fee, state: "listed" }), /until block 120,016/);
});
