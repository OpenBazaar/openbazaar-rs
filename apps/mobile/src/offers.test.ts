import assert from "node:assert/strict";
import { test } from "node:test";

import { answerSubmission, offerSubmission, parseWaitingBids, OFFER_ROUTE } from "./offers.ts";

const draft = {
  listingHash: "ab".repeat(32),
  variant: "01".repeat(8),
  quantity: 2,
  amount: "18.00",
  currency: "USD" as const,
};

test("the make-an-offer sheet posts one bid to the offer route, and an empty amount is refused", () => {
  const sent = offerSubmission(draft);
  assert.equal(sent.path, OFFER_ROUTE);
  assert.equal(sent.path, "/offers");
  assert.equal(sent.path.includes("mail"), false);
  assert.equal(sent.path.includes("checkout"), false);
  assert.deepEqual(sent.body, {
    listingHash: draft.listingHash,
    variant: draft.variant,
    quantity: 2,
    amount: 1_800,
    currency: "USD",
  });
  const text = JSON.stringify(sent.body);
  assert.equal(text.includes("buyer"), false);
  assert.equal(text.includes("reply"), false);
  assert.equal(text.includes("address"), false);
  for (const amount of ["", "  ", "0", "0.00", "nope"]) {
    assert.throws(() => offerSubmission({ ...draft, amount }), /Enter an amount/);
  }
  const answer = answerSubmission("ab".repeat(16), { expiresAt: 1_800_000_000, amount: 1_700 });
  assert.equal(answer.path, `/offers/${"ab".repeat(16)}/answer`);
  assert.equal(answer.path.startsWith("/offers/"), true);
  assert.equal(answer.path.endsWith("/answer"), true);
});

test("a waiting bid keeps the price and drops the conversation key", () => {
  const [bid] = parseWaitingBids({
    offers: [{
      id: "cd".repeat(16),
      listingId: "ef".repeat(16),
      variantId: "00".repeat(8),
      quantity: 1,
      amount: 1_800,
      currency: "USD",
      buyerKey: "11".repeat(32),
      replyUrl: "http://127.0.0.1:9",
      drop: "22".repeat(16),
    }],
  });
  assert.deepEqual(Object.keys(bid).sort(), ["amount", "currency", "id", "listingId", "quantity", "variantId"]);
  assert.equal(JSON.stringify(bid).includes("buyer"), false);
  assert.equal(JSON.stringify(bid).includes("http"), false);
});
