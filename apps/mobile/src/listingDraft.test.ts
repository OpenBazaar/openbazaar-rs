import assert from "node:assert/strict";
import { test } from "node:test";

import { draftListingId, draftPath } from "./listingDraft.ts";

test("a draft of a published listing keeps that listing's id", () => {
  assert.equal(draftListingId(null, null), null);
  assert.equal(draftListingId(null, { listing: {} }), null);
  assert.equal(draftListingId({ listingId: "b488" }, null), "b488");
  assert.equal(draftListingId(null, { listing: { listingId: "b488" } }), "b488");
});

test("saving a draft does not use the listing sign path", () => {
  assert.equal(draftPath(null), "/store/drafts");
  assert.equal(draftPath("abc"), "/store/drafts/abc");
  assert.equal(draftPath(null).includes("/store/listings"), false);
  assert.equal(draftPath("abc").includes("/store/listings"), false);
});
