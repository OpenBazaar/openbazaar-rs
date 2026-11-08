import assert from "node:assert/strict";
import { test } from "node:test";

import { draftPath } from "./listingDraft.ts";

test("saving a draft does not use the listing sign path", () => {
  assert.equal(draftPath(null), "/store/drafts");
  assert.equal(draftPath("abc"), "/store/drafts/abc");
  assert.equal(draftPath(null).includes("/store/listings"), false);
  assert.equal(draftPath("abc").includes("/store/listings"), false);
});
