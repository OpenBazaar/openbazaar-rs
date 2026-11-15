import assert from "node:assert/strict";
import test from "node:test";
import { parseMessages } from "./messages.ts";

test("order messages keep only buyer/seller plain text entries", () => {
  const messages = parseMessages([
    { id: "a".repeat(32), from: "buyer", text: "Is this still available?", at: 10 },
    { id: "b".repeat(32), from: "seller", text: "Yes — shipping Monday.", at: 20 },
    { id: "c".repeat(32), from: "moderator", text: "intruder", at: 30 },
    { id: "d".repeat(32), from: "buyer", text: "", at: 40 },
    "garbage",
  ]);
  assert.deepEqual(messages.map(m => m.from), ["buyer", "seller"]);
  assert.equal(messages[1].text, "Yes — shipping Monday.");
});

test("order messages refuse non-lists", () => {
  assert.deepEqual(parseMessages(null), []);
  assert.deepEqual(parseMessages({ messages: [] }), []);
});
