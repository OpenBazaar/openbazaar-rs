import assert from "node:assert/strict";
import { test } from "node:test";

import { axesProblem, combinations, emptyCommerce, forKind, parseValues, rowsOf, syncRows, withRows } from "./commerceDraft.ts";

const size = { name: "Size", values: ["S", "M"] };
const color = { name: "Color", values: ["Navy", "Sand"] };

test("every combination gets a row and existing rows keep their id", () => {
  assert.deepEqual(combinations([size, color]), [["S", "Navy"], ["S", "Sand"], ["M", "Navy"], ["M", "Sand"]]);
  const rows = syncRows([size, color], []).map((row, i) => ({ ...row, id: `id${i}`, stock: i }));
  const grown = syncRows([{ ...size, values: ["S", "M", "L"] }, color], rows);
  assert.equal(grown.length, 6);
  assert.equal(grown.find(r => r.options.join() === "M,Sand")?.id, "id3");
  assert.equal(grown.find(r => r.options.join() === "L,Navy")?.id, "");
});

test("rows switched off are left out of the signed listing", () => {
  const rows = syncRows([size, color], []).map((row, i) => ({ ...row, offered: i !== 1, stock: 2 }));
  const commerce = withRows(emptyCommerce(), [size, color], rows);
  assert.equal(commerce.variants.length, 3);
  assert.ok(!commerce.variants.some(v => v.options.join() === "S,Sand"));
  assert.deepEqual(rowsOf(commerce).map(r => r.offered), [true, false, true, true]);
});

test("the editor names what to fix", () => {
  assert.equal(axesProblem([{ name: "", values: ["S"] }], []), "Name each option, like Size or Color.");
  assert.equal(axesProblem([size, { ...size }], []), "\"Size\" is used twice.");
  const off = syncRows([size], []).map(r => ({ ...r, offered: false }));
  assert.equal(axesProblem([size], off), "Offer at least one combination.");
  assert.equal(axesProblem([size], syncRows([size], [])), null);
  assert.deepEqual(parseValues(" S, m ,M,, L "), ["S", "m", "L"]);
});

test("a digital listing carries no shipping", () => {
  const physical = { ...emptyCommerce(), shipping: { ...emptyCommerce().shipping, pickup: "Oregon", handlingDays: 2 } };
  const digital = forKind(physical, "digital");
  assert.deepEqual(digital.shipping, { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 0, freeOver: 0, services: [], pickup: null });
});
