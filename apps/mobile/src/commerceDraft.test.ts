import assert from "node:assert/strict";
import { test } from "node:test";

import { axesProblem, combinations, compareAtText, defaultService, emptyCommerce, forKind, listingFacts, packageFieldsOf, parseValues, priceTerms, purchaseFields, purchaseTerms, returnsDraft, rowsOf, saleFieldsOf, scheduledSale, shippingTerms, signedTaxonomy, signedWeight, specificRows, syncRows, tierRows, volumeTiers, withFacts, withPackage, withPrice, withPurchase, withRows, withSale, withTaxonomy, withTiers, withVariantWeights } from "./commerceDraft.ts";

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

test("the compare-at and offers flag the editor saves are signed onto the listing, and an invalid compare-at is refused", () => {
  const saved = priceTerms(2_500, "USD", "32.00", true);
  assert.deepEqual(saved, { compareAt: 3_200, acceptsOffers: true });
  if (!saved || typeof saved === "string") throw new Error("the price terms were refused");
  const commerce = withPrice(null, saved);
  assert.equal(commerce?.compareAt, 3_200);
  assert.equal(commerce?.acceptsOffers, true);
  assert.equal(compareAtText(commerce?.compareAt ?? 0, "USD"), "32.00");
  assert.equal(priceTerms(2_500, "USD", "25.00", true), "A compare-at price is above every price it is compared with.");
  assert.equal(priceTerms(2_500, "USD", "20.00", false), "A compare-at price is above every price it is compared with.");
  assert.equal(priceTerms(2_500, "USD", "30.00", false, [3_200]), "A compare-at price is above every price it is compared with.");
  assert.equal(priceTerms(2_500, "USD", "", "yes"), "An offers flag is yes or no.");
  assert.deepEqual(priceTerms(2_500, "USD", "", false), { compareAt: 0, acceptsOffers: false });
  assert.equal(withPrice(null, { compareAt: 0, acceptsOffers: false }), null);
});

test("the availability and lot size the editor saves are signed onto the listing, and an invalid value is refused", () => {
  const saved = purchaseTerms("5", { kind: "pre-order", shipsOn: "2026-12-01", days: "" });
  assert.deepEqual(saved, { lotSize: 5, availability: { kind: "pre-order", shipsOn: 20261201 } });
  if (!saved || typeof saved === "string") throw new Error("the purchase terms were refused");
  const commerce = withPurchase(null, saved);
  assert.equal(commerce?.lotSize, 5);
  assert.deepEqual(commerce?.availability, { kind: "pre-order", shipsOn: 20261201 });
  assert.deepEqual(purchaseFields(commerce), { lotSize: "5", availability: { kind: "pre-order", shipsOn: "2026-12-01", days: "" } });
  assert.equal(purchaseTerms("0", { kind: "in-stock", shipsOn: "", days: "" }), "A unit is at least one piece.");
  assert.equal(purchaseTerms("65536", { kind: "in-stock", shipsOn: "", days: "" }), "The lot size must be a whole number up to 65535.");
  assert.equal(purchaseTerms("1", { kind: "paused", shipsOn: "", days: "" }), "Availability is in-stock, pre-order, made-to-order, or away.");
  assert.equal(purchaseTerms("1", { kind: "made-to-order", shipsOn: "", days: "400" }), "The making time must be a whole number up to 365.");
  assert.equal(purchaseTerms("1", { kind: "pre-order", shipsOn: "2026-13-01", days: "" }), "The pre-order ship date must be a date written YYYYMMDD.");
  assert.deepEqual(purchaseTerms("", { kind: "in-stock", shipsOn: "", days: "" }), { lotSize: 1, availability: { kind: "in-stock" } });
  assert.equal(withPurchase(null, { lotSize: 1, availability: { kind: "in-stock" } }), null);
});

test("the per-variant weight and volume tiers the editor saves are signed onto the listing, and an invalid value is refused", () => {
  const variant = { id: "", options: ["Mug"], sku: "", price: 0, stock: 2, image: 0, weightG: 0 };
  assert.equal(signedWeight("220"), 220);
  const weighted = withVariantWeights([variant], ["220"]);
  if (typeof weighted === "string") throw new Error("the weight was refused");
  assert.equal(weighted[0].weightG, 220);
  const saved = volumeTiers(2_500, "USD", [{ qty: "3", price: "20.00" }, { qty: "", price: "" }]);
  assert.deepEqual(saved, [{ minQty: 3, offBps: 2_000 }]);
  if (typeof saved === "string") throw new Error("the tiers were refused");
  const commerce = withTiers(null, saved);
  assert.deepEqual(commerce?.tiers, saved);
  assert.equal(commerce?.variants.length, 0);
  const withWeight = commerce ? { ...commerce, variants: weighted } : null;
  assert.equal(withWeight?.variants[0].weightG, 220);
  assert.deepEqual(tierRows(2_500, saved, "USD"), [{ qty: "3", price: "20.00" }]);
  assert.equal(signedWeight("4294967296"), "Variant weight is a whole number of grams.");
  assert.equal(signedWeight("1.5"), "Variant weight is a whole number of grams.");
  assert.equal(volumeTiers(2_500, "USD", [{ qty: "3", price: "" }]), "A volume tier is empty.");
  assert.equal(volumeTiers(2_500, "USD", [{ qty: "", price: "20.00" }]), "A volume tier is empty.");
  assert.equal(volumeTiers(2_500, "USD", [{ qty: "3", price: "25.00" }]), "A volume tier price is below the regular price.");
  assert.equal(volumeTiers(2_500, "USD", [{ qty: "1", price: "20.00" }]), "A volume tier starts at 2 and takes 0.01% to 90% off.");
  assert.equal(volumeTiers(2_500, "USD", [{ qty: "5", price: "20.00" }, { qty: "3", price: "15.00" }]), "Volume tiers rise in quantity and discount.");
  assert.equal(withTiers(null, []), null);
});

test("the taxonomy the editor saves is signed onto the listing, and an invalid taxonomy is refused", () => {
  assert.equal(signedTaxonomy("3"), 3);
  const commerce = withTaxonomy(null, 3);
  assert.equal(commerce?.taxonomy, 3);
  assert.equal(signedTaxonomy(""), 0);
  assert.equal(signedTaxonomy("0"), 0);
  assert.equal(withTaxonomy(null, 0), null);
  assert.equal(signedTaxonomy("65536"), "The category number must be a whole number up to 65535.");
  assert.equal(signedTaxonomy("clothing"), "The category number must be a whole number up to 65535.");
});

test("a digital listing carries no shipping", () => {
  const physical = { ...emptyCommerce(), shipping: { ...emptyCommerce().shipping, pickup: "Oregon", handlingDays: 2 } };
  const digital = forKind(physical, "digital");
  assert.deepEqual(digital.shipping, { weightG: 0, dimsMm: [0, 0, 0], handlingDays: 0, freeOver: 0, services: [], pickup: null });
});
