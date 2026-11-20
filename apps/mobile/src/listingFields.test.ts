import assert from "node:assert/strict";
import { test } from "node:test";
import { CATEGORY_NAMES, categoryFieldsOf, categoryLabel, filledTiers, sameChoice, signedCategory, topCategory } from "./listingFields.ts";

test("a fiat listing signs the category number and keeps the subcategory as its text", () => {
  assert.deepEqual(signedCategory({ taxonomy: "5", subcategory: " Wool blankets " }, true), { category: "Wool blankets", taxonomy: 5 });
  assert.deepEqual(signedCategory({ taxonomy: "5", subcategory: "" }, true), { category: "Home and kitchen", taxonomy: 5 });
});

test("a sats listing signs the category by name", () => {
  assert.deepEqual(signedCategory({ taxonomy: "13", subcategory: "Corn dogs" }, false), { category: "Food and drink", taxonomy: 0 });
});

test("a listing without a known category is refused", () => {
  for (const taxonomy of ["", "0", "99", "x"]) assert.equal(signedCategory({ taxonomy, subcategory: "Mugs" }, true), "Choose a category.");
  assert.equal(signedCategory({ taxonomy: "5", subcategory: "x".repeat(41) }, true), "A subcategory is at most 40 characters.");
});

test("category fields round-trip what was signed", () => {
  assert.deepEqual(categoryFieldsOf("Wool blankets", 5), { taxonomy: "5", subcategory: "Wool blankets" });
  assert.deepEqual(categoryFieldsOf("Home and kitchen", 5), { taxonomy: "5", subcategory: "" });
  assert.deepEqual(categoryFieldsOf("food and drink", 0), { taxonomy: "13", subcategory: "" });
  assert.deepEqual(categoryFieldsOf("Home", 0), { taxonomy: "5", subcategory: "" });
  assert.deepEqual(categoryFieldsOf("Grocery", 0), { taxonomy: "13", subcategory: "" });
});

test("buyers browse a listing under its fixed category when it has one", () => {
  assert.equal(topCategory({ category: "Wool blankets", commerce: { taxonomy: 5 } }), "Home and kitchen");
  assert.equal(topCategory({ category: "books", commerce: null }), "Books");
  assert.equal(topCategory({ category: "Grocery" }), "Food and drink");
  assert.equal(topCategory({ category: "Grocery", commerce: { taxonomy: 999 } }), "Food and drink");
});

test("a seller's own text never becomes a navigation entry", () => {
  const hostile = ["Wool blankets", "x".repeat(60_000), "\u202eevil", "constructor", "__proto__", "toString", "", "Books\n\nBUY NOW"];
  for (const category of hostile) assert.ok(CATEGORY_NAMES.includes(topCategory({ category, commerce: { taxonomy: 999 } })), category);
  assert.equal(topCategory({ category: "constructor" }), "Other");
});

test("the category label is cleaned and bounded", () => {
  assert.equal(categoryLabel({ category: "  Wool\u202e blankets\n" }), "Wool blankets");
  assert.equal(categoryLabel({ category: "\u200b\u200b" }), "Other");
  assert.ok([...categoryLabel({ category: "x".repeat(60_000) })].length <= 40);
});

test("choices match however they were stored, and blank tier rows are dropped", () => {
  assert.ok(sameChoice("Country of origin", "country-of-origin"));
  assert.ok(!sameChoice("Brand", "model"));
  assert.deepEqual(filledTiers([{ qty: "", price: "" }, { qty: "3", price: "" }]), [{ qty: "3", price: "" }]);
});
