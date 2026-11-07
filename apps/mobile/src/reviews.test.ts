import assert from "node:assert/strict";
import { test } from "node:test";

import { forListing, parseStoreReviews, reviewDate } from "./reviews.ts";

const body = {
  count: 3, average: 3.6, stars: [0, 1, 0, 1, 1],
  reviews: [
    { id: "a1", listingId: "mug", listingTitle: "Mug", rating: 5, text: "Lovely.", writtenOn: 20261110, paidMonth: 202611, reply: null },
    { id: "a2", listingId: "mug", listingTitle: "Mug", rating: 2, text: "Chipped.", writtenOn: 20261108, paidMonth: 202611, reply: { text: "Sorry, sending another.", writtenOn: 20261109 } },
    { id: "a3", listingId: "bowl", listingTitle: null, rating: 4, text: "", writtenOn: 20261101, paidMonth: 202610, reply: null },
    { id: "bad", listingId: "mug", rating: 9, text: "Not a rating." },
  ],
};

test("reviews keep their reply and drop what is not a review", () => {
  const all = parseStoreReviews(body);
  assert.equal(all.count, 3);
  assert.equal(all.reviews[1].reply?.text, "Sorry, sending another.");
  assert.equal(all.average, 3.6);
});

test("one listing's stars are counted from its own reviews", () => {
  const mug = forListing(parseStoreReviews(body), "mug");
  assert.deepEqual([mug.count, mug.average, mug.stars], [2, 3.5, [0, 1, 0, 0, 1]]);
  assert.equal(forListing(parseStoreReviews(body), "cup").average, null);
});

test("a receipt's month and a review's day read plainly", () => {
  assert.equal(reviewDate(202611), "Nov 2026");
  assert.equal(reviewDate(20261112), "Nov 12, 2026");
});
