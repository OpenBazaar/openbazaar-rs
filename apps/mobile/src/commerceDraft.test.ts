import assert from "node:assert/strict";
import { test } from "node:test";

import { axesProblem, combinations, defaultService, emptyCommerce, forKind, listingFacts, packageFieldsOf, parseValues, returnsDraft, rowsOf, saleFieldsOf, scheduledSale, shippingTerms, specificRows, syncRows, withFacts, withPackage, withRows, withSale } from "./commerceDraft.ts";

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

test("the sale the editor saves is signed onto the listing, and an invalid window is refused", () => {
  const saved = scheduledSale(2_500, "USD", { price: "20.00", startsOn: "2026-11-01", endsOn: "2026-11-30" });
  assert.deepEqual(saved, { offBps: 2_000, startsOn: 20261101, endsOn: 20261130 });
  if (!saved || typeof saved === "string") throw new Error("the sale was refused");
  const commerce = withSale(null, saved);
  assert.deepEqual(commerce?.sale, saved);
  assert.deepEqual(saleFieldsOf(2_500, commerce?.sale ?? null, "USD"), { price: "20.00", startsOn: "2026-11-01", endsOn: "2026-11-30" });
  assert.equal(scheduledSale(2_500, "USD", { price: "", startsOn: "", endsOn: "" }), null);
  assert.equal(scheduledSale(2_500, "USD", { price: "", startsOn: "2026-11-01", endsOn: "2026-11-30" }), "Enter a sale price.");
  assert.equal(scheduledSale(2_500, "USD", { price: "  ", startsOn: "2026-11-01", endsOn: "2026-11-30" }), "Enter a sale price.");
  assert.equal(scheduledSale(2_500, "USD", { price: "0.00", startsOn: "2026-11-01", endsOn: "2026-11-30" }), "Enter a sale price.");
  assert.equal(scheduledSale(2_500, "USD", { price: "30.00", startsOn: "2026-11-01", endsOn: "2026-11-30" }), "A sale price is below the regular price.");
  assert.equal(scheduledSale(2_500, "USD", { price: "20.00", startsOn: "2026-11-30", endsOn: "2026-11-01" }), "A sale ends before it starts.");
});

test("the package the editor saves is signed onto the listing, and an invalid handling time is refused", () => {
  const saved = shippingTerms("USD", {
    weightG: "450", lengthMm: "120", widthMm: "80", heightMm: "60", handlingDays: "2", freeOver: "50.00", regions: ["us, CA"],
  });
  assert.deepEqual(saved, { weightG: 450, dimsMm: [120, 80, 60], handlingDays: 2, freeOver: 5_000, regions: [["CA", "US"]] });
  if (!saved || typeof saved === "string") throw new Error("the package was refused");
  const commerce = withPackage({ ...emptyCommerce(), shipping: { ...emptyCommerce().shipping, services: [defaultService()] } }, saved);
  assert.equal(commerce?.shipping.weightG, 450);
  assert.deepEqual(commerce?.shipping.dimsMm, [120, 80, 60]);
  assert.equal(commerce?.shipping.handlingDays, 2);
  assert.equal(commerce?.shipping.freeOver, 5_000);
  assert.deepEqual(commerce?.shipping.services[0].regions, ["CA", "US"]);
  assert.equal(packageFieldsOf(commerce?.shipping, "USD").handlingDays, "2");
  assert.equal(shippingTerms("USD", { weightG: "450", lengthMm: "120", widthMm: "80", heightMm: "60", handlingDays: "31", freeOver: "", regions: [""] }), "Handling time is at most 30 days.");
  assert.equal(shippingTerms("USD", { weightG: "", lengthMm: "", widthMm: "", heightMm: "", handlingDays: "2", freeOver: "", regions: ["USA"] }), "A region is a two-letter country code.");
});

test("the specifics and returns the editor saves are signed onto the listing, and an invalid value is refused", () => {
  const saved = listingFacts(
    [{ key: "Brand", value: "Marlow" }, { key: "UPC", value: "036000291452" }, { key: "", value: "" }],
    { kind: "days", days: "30", sellerPays: true },
  );
  assert.deepEqual(saved, { specifics: { brand: "Marlow", upc: "036000291452" }, returns: { kind: "days", days: 30, sellerPays: true } });
  if (!saved || typeof saved === "string") throw new Error("the facts were refused");
  const commerce = withFacts(null, saved);
  assert.deepEqual(commerce?.specifics, saved.specifics);
  assert.deepEqual(commerce?.returns, saved.returns);
  assert.deepEqual(specificRows(commerce?.specifics), [{ key: "brand", value: "Marlow" }, { key: "upc", value: "036000291452" }]);
  assert.deepEqual(returnsDraft(commerce?.returns), { kind: "days", days: "30", sellerPays: true });
  assert.equal(listingFacts([{ key: "UPC", value: "036000291453" }], { kind: "store", days: "", sellerPays: false }), "A product code's check digit is wrong.");
  assert.equal(listingFacts([], { kind: "days", days: "0", sellerPays: false }), "A returns window is 1 to 365 days.");
  assert.equal(listingFacts([], { kind: "days", days: "400", sellerPays: false }), "A returns window is 1 to 365 days.");
  assert.equal(listingFacts([{ key: "price!", value: "12" }], { kind: "store", days: "", sellerPays: false }), "An item specific key is lowercase letters, digits and dashes.");
});

test("a digital listing carries no shipping", () => {
  const physical = { ...emptyCommerce(), shipping: { ...emptyCommerce().shipping, pickup: "Oregon", handlingDays: 2 } };
  const digital = forKind(physical, "digital");
  assert.deepEqual(digital.shipping, { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 0, freeOver: 0, services: [], pickup: null });
});
