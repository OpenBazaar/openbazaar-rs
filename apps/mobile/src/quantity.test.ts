import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { canOrder, checkoutLine, nextQuantity } from "./quantity.ts";

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");
const shop = read("./marketplace/Marketplace.tsx");
const commerce = read("./marketplace/Commerce.tsx");
const picker = read("./marketplace/VariantPicker.tsx");
const phone = read("../App.tsx");
const css = read("../public/listings.css");

test("the shop shows a quantity and checkout keeps the chosen count", () => {
  const stepper = picker.slice(picker.indexOf("export function QuantityStepper"));
  assert.match(stepper, /className="ob-buy-quantity"/);
  assert.match(stepper, /aria-label="Quantity">\{quantity\}<\/output>/);
  assert.match(stepper, /aria-label="One fewer"/);
  assert.match(stepper, /aria-label="One more"/);
  assert.doesNotMatch(stepper.slice(0, stepper.indexOf("export function PriceTag")), /display:\s*none|hidden/);
  assert.match(shop, /<QuantityStepper quantity=\{buy\.quantity\} max=\{buy\.max\} onQuantity=\{buy\.setQuantity\}/);
  assert.match(commerce, /aria-label=\{`Quantity for /);
  assert.match(phone, /label="Quantity"/);
  assert.match(phone, /accessibilityLabel=\{`Quantity for \$\{item\.listing\.title\}`\}/);
  assert.doesNotMatch(css, /\.ob-buy-quantity\s*\{[^}]*display:\s*none/);

  let quantity = 1;
  assert.deepEqual(checkoutLine({ listingHash: "ab", quantity }), { listingHash: "ab", quantity: 1 });
  quantity = nextQuantity(quantity, 1, 8);
  assert.equal(quantity, 2);
  assert.deepEqual(checkoutLine({ listingHash: "ab", quantity, variant: "v" }), { listingHash: "ab", quantity: 2, variant: "v" });
  assert.match(commerce, /checkoutLine\(\{ listingHash: i\.listing\.hash, quantity: i\.quantity, variant: i\.variant \}\)/);
  assert.match(phone, /checkoutLine\(\{ listingHash: item\.listing\.hash, quantity: item\.quantity, variant: item\.variant \}\)/);
  assert.match(commerce, /!canOrder\(i\.quantity, i\.available\)/);
  assert.match(phone, /!canOrder\(item\.quantity, item\.available\)/);

  assert.equal(canOrder(0, 8), false);
  assert.equal(canOrder(-1, 8), false);
  assert.equal(canOrder(9, 8), false);
  assert.equal(canOrder(1.5, 8), false);
  assert.equal(canOrder(100, Number.POSITIVE_INFINITY), false);
  assert.equal(canOrder(1, 8), true);
  assert.equal(canOrder(8, 8), true);
  assert.equal(canOrder(3, Number.POSITIVE_INFINITY), true);
  assert.equal(nextQuantity(1, -1, 8), 1);
  assert.equal(nextQuantity(8, 1, 8), 8);
  assert.match(stepper, /if \(!\(max > 0\)\) return null/);
});
