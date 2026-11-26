import assert from "node:assert/strict";
import { test } from "node:test";

import {
  CLASSIFIEDS_ROUTE,
  classifiedSubmission,
  parseClassifieds,
  parseClassifiedsPage,
  parseContactThread,
  parseContactThreads,
} from "./classifieds.ts";

test("a classified post is trimmed and refuses empty text or an overlong life", () => {
  const sent = classifiedSubmission({
    kind: "wtb",
    category: "goods",
    title: " Looking for chain ",
    body: " 11-speed preferred. ",
    lifeDays: 7,
    region: "Oregon",
    priceHint: "2500",
    currency: "usd",
  });
  assert.equal(sent.path, CLASSIFIEDS_ROUTE);
  assert.deepEqual(sent.body, {
    kind: "wtb",
    category: "goods",
    title: "Looking for chain",
    body: "11-speed preferred.",
    lifeDays: 7,
    region: "Oregon",
    priceHint: 2500,
    currency: "USD",
  });
  assert.throws(() => classifiedSubmission({ kind: "wts", category: "other", title: " ", body: "hi" }), /title/);
  assert.throws(() => classifiedSubmission({ kind: "wts", category: "other", title: "hi", body: "x", lifeDays: 40 }), /30/);
});

test("board and contact payloads parse into typed rows", () => {
  const posts = parseClassifieds({
    classifieds: [
      {
        id: "11".repeat(16),
        kind: "wts",
        category: "housing",
        title: "Spare room",
        body: "Quiet street.",
        region: "Portland",
        priceHint: 0,
        currency: "",
        createdOn: 20261125,
        expiresOn: 20261209,
        lifeDays: 14,
        expired: false,
        poster: "22".repeat(32),
        mailboxKey: "33".repeat(32),
        onion: "",
      },
    ],
  });
  assert.equal(posts.length, 1);
  assert.equal(posts[0].kind, "wts");
  assert.equal(posts[0].category, "housing");

  const page = parseClassifiedsPage({
    classifieds: [
      {
        id: "11".repeat(16),
        kind: "wts",
        category: "housing",
        title: "Spare room",
        body: "Quiet street.",
        region: "Portland",
        priceHint: 0,
        currency: "",
        createdOn: 20261125,
        expiresOn: 20261209,
        lifeDays: 14,
        expired: false,
        poster: "22".repeat(32),
        mailboxKey: "33".repeat(32),
        onion: "",
      },
    ],
    total: 40,
    limit: 25,
    offset: 25,
  });
  assert.equal(page.total, 40);
  assert.equal(page.limit, 25);
  assert.equal(page.offset, 25);
  assert.equal(page.classifieds.length, 1);

  const threads = parseContactThreads({
    threads: [
      {
        id: "44".repeat(16),
        classifiedId: "11".repeat(16),
        poster: "22".repeat(32),
        peer: "55".repeat(32),
        asPoster: true,
        title: "Spare room",
        messages: [{ id: "66".repeat(16), from: "them", text: "Still open?", at: 1 }],
      },
    ],
  });
  assert.equal(threads[0].messages[0].from, "them");
  assert.equal(parseContactThread(threads[0]).title, "Spare room");
});
