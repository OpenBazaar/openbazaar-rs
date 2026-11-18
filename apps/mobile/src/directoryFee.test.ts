import assert from "node:assert/strict";
import { test } from "node:test";

import { directoryFeeChanges, directoryFeeOf, directoryFeeStatus, directoryHeadRefusal, epochWords } from "./directoryFee.ts";

const address = "tb1p" + "q".repeat(58);
const other = "tb1p" + "z".repeat(58);

test("the fee keeps the address, amount, and a known state", () => {
  assert.deepEqual(directoryFeeOf({ available: true, address, sats: 20000, confirmations: 1, epochBlocks: 2016, state: "listed", listedUntilHeight: 120016, error: null }), {
    available: true, address, sats: 20000, confirmations: 1, epochBlocks: 2016, state: "listed", listedUntilHeight: 120016, upcoming: [], headRefused: null,
  });
  const odd = directoryFeeOf({ available: true, address, sats: 20000, state: "paid?" });
  assert.equal(odd.available && odd.state, "checking");
});

test("an unavailable or unreadable answer gives a reason, never an address", () => {
  assert.deepEqual(directoryFeeOf({ available: false, reason: "The directory fee runs on testnet4 only." }), { available: false, reason: "The directory fee runs on testnet4 only." });
  for (const unreadable of [null, "fee", {}, { available: true, sats: 20000 }]) assert.equal(directoryFeeOf(unreadable).available, false);
});

const fee = { available: true as const, address, sats: 20000, confirmations: 1, epochBlocks: 2016, listedUntilHeight: 120016, upcoming: [], headRefused: null };

test("the status says what the seller does next", () => {
  assert.equal(directoryFeeStatus({ ...fee, state: "unpaid" }), "Not listed yet. Your node could not pay from its bitcoin wallet. Send 20,000 sats to this address from any wallet, exchange, or swap.");
  assert.equal(directoryFeeStatus({ ...fee, state: "confirming" }), "Payment seen. Your store is listed after 1 confirmation.");
  assert.equal(directoryFeeStatus({ ...fee, state: "confirming", confirmations: 3 }), "Payment seen. Your store is listed after 3 confirmations.");
  assert.match(directoryFeeStatus({ ...fee, state: "listed" }), /until block 120,016/);
});

test("the epoch reads in words", () => {
  assert.equal(epochWords(2016), "two weeks");
  assert.equal(epochWords(1008), "1 week");
  assert.equal(epochWords(4032), "4 weeks");
  assert.equal(epochWords(432), "3 days");
  assert.equal(epochWords(1000), "1000 blocks");
});

test("scheduled terms are read and each change is said against the terms before it", () => {
  const read = directoryFeeOf({
    ...fee,
    state: "listed",
    upcoming: [
      { startHeight: 125000, announcedHeight: 123000, address, addressChanges: false, sats: 30000, confirmations: 1, epochBlocks: 2016 },
      { startHeight: 130000, announcedHeight: 128000, address: other, addressChanges: true, sats: 25000, confirmations: 2, epochBlocks: 4032 },
      { startHeight: 131000, announcedHeight: 130500, address: other, addressChanges: true, sats: 25000, confirmations: 2, epochBlocks: 4032 },
      { startHeight: "soon", sats: 1 },
    ],
  });
  assert.ok(read.available);
  assert.equal(read.upcoming.length, 3, "an unreadable entry is dropped");
  assert.deepEqual(directoryFeeChanges(read), [
    "From block 125,000, the fee rises to 30,000 sats. The directory announced this at block 123,000.",
    "From block 130,000, the fee drops to 25,000 sats, a payment needs 2 confirmations, each payment lists the store for 4 weeks, and the fee address changes. The directory announced this at block 128,000.",
    "From block 131,000, the fee address changes. The directory announced this at block 130,500.",
  ]);
  assert.deepEqual(directoryFeeChanges({ ...fee, state: "listed", upcoming: [{ ...read.upcoming[0], sats: 20000 }] }), [
    "From block 125,000, the terms stay the same. The directory announced this at block 123,000.",
  ]);
});

test("a refused directory update is shown with its reason", () => {
  assert.equal(directoryHeadRefusal({ ...fee, state: "listed" }), null);
  const read = directoryFeeOf({ ...fee, state: "listed", headRefused: { sequence: 9, reason: "it edits terms that already started" } });
  assert.ok(read.available);
  assert.equal(directoryHeadRefusal(read), "Your node refused directory update 9: it edits terms that already started. It keeps the terms shown here.");
  const unreadable = directoryFeeOf({ ...fee, state: "listed", headRefused: { sequence: "9" } });
  assert.equal(unreadable.available && unreadable.headRefused, null);
});
