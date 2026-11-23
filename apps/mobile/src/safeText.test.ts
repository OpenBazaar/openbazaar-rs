import assert from "node:assert/strict";
import { test } from "node:test";
import { displayText, plainLabel } from "./safeText.ts";

test("ordinary text passes unchanged", () => {
  assert.equal(displayText("Wool blankets", 40), "Wool blankets");
  assert.equal(displayText("Café crème", 40), "Café crème");
  assert.equal(displayText("日本茶", 40), "日本茶");
});

test("direction overrides, isolates, and invisible characters are removed", () => {
  assert.equal(displayText("abc\u202Egpj.exe", 40), "abc gpj.exe");
  assert.equal(displayText("a\u2066b\u2069c", 40), "a b c");
  assert.equal(displayText("in\u200bvis\ufeffible", 40), "in vis ible");
  assert.equal(displayText("\udb40\udc41tag", 40), "tag");
});

test("controls and line breaks collapse to single spaces", () => {
  assert.equal(displayText("  Books\n\n\tBUY\u0000NOW\u2028  ", 40), "Books BUY NOW");
  assert.equal(displayText("\u0007\u009b", 40), "");
});

test("a tower of combining marks is cut to two per letter", () => {
  const zalgo = "Z" + "\u0336\u0337\u0338\u0339".repeat(50) + "a";
  assert.equal(displayText(zalgo, 40), "Z\u0336\u0337a");
  assert.equal(displayText("e\u0301", 40), "e\u0301");
});

test("a filter label drops direction overrides and keeps emoji", () => {
  assert.equal(plainLabel("Marlow Goods"), "Marlow Goods");
  assert.equal(plainLabel("Oregon"), "Oregon");
  assert.equal(plainLabel("Shop\u{202e}evil"), "Shopevil");
  assert.equal(plainLabel("a\u{2066}b\u{2069}c"), "abc");
  assert.equal(plainLabel("Maine\n"), "Maine");
  assert.equal(plainLabel("Soft 👨‍👩‍👧 ❤️"), "Soft 👨‍👩‍👧 ❤️");
});

test("long text is cut to the limit with an ellipsis", () => {
  const out = displayText("x".repeat(100_000), 40);
  assert.equal([...out].length, 40);
  assert.ok(out.endsWith("…"));
  assert.equal([...displayText("😀".repeat(100), 10)].length, 10);
});
