import assert from "node:assert/strict";
import { test } from "node:test";

import { cartPackages, lineKey } from "./cart.ts";
import type { ListingHeader, ShopCatalog } from "./catalog.ts";
import { emptyCommerce } from "./commerceDraft.ts";

const glaze = { name: "Glaze", values: ["Sea", "Sand"] };
const post = { name: "Post", price: 500, extraItem: 100, daysMin: 3, daysMax: 5, regions: [] };

function mug(sandStock = 3): ListingHeader {
  const commerce = {
    ...emptyCommerce(),
    axes: [glaze],
    variants: [
      { id: "00000000000000a1", options: ["Sea"], sku: "", price: 2500, stock: 2, image: 0, weightG: 0 },
      { id: "00000000000000a2", options: ["Sand"], sku: "", price: 3000, stock: sandStock, image: 0, weightG: 0 },
    ],
    shipping: { ...emptyCommerce().shipping, services: [post], pickup: "Portland, OR" },
  };
  return {
    hash: "aa".repeat(32), storeId: "bb".repeat(32), title: "Mug", category: "Home", priceSats: 50_000, shipFrom: "US", kind: "physical",
    acceptsModerator: false, acceptsXmr: false, seller: "Clay", stock: 5, listedOn: 20260101, product: "", shippingSats: 0, freeOverSats: 0,
    contentRating: "general", contentTags: [], listingId: "cc".repeat(16), images: [], price: { currency: "USD", amount: 2500, shipping: 0 }, commerce,
  } as ListingHeader;
}

const shop = (listing: ListingHeader) => ({ listings: [listing], moderator: null }) as unknown as ShopCatalog;
const sea = { hash: "aa".repeat(32), variant: "00000000000000a1", quantity: 2 };
const sand = { hash: "aa".repeat(32), variant: "00000000000000a2", quantity: 1 };

test("two variants of one listing are two bag lines priced in sats at the signed rate", () => {
  const [pkg] = cartPackages(shop(mug()), [sea, sand]);
  assert.deepEqual(pkg.items.map(i => [i.key, i.label, i.unitSats, i.available]), [
    [lineKey(sea), "Sea", 50_000, 2],
    [lineKey(sand), "Sand", 60_000, 3],
  ]);
  assert.equal(pkg.itemsSats, 160_000);
  assert.equal(pkg.shippingSats, 14_000, "Post is $5 plus $1 for each extra item");
  assert.equal(pkg.quote?.total, 8700);
  assert.equal(pkg.problem, "");
});

test("local pickup drops shipping", () => {
  const [pkg] = cartPackages(shop(mug()), [sea, sand], { Clay: -1 });
  assert.equal(pkg.shippingSats, 0);
  assert.equal(pkg.totalSats, 160_000);
});

test("a sold-out variant shows no units left, which holds the bag back from checkout", () => {
  const [pkg] = cartPackages(shop(mug(0)), [sand]);
  assert.equal(pkg.items[0].available, 0);
  assert.ok(pkg.items[0].quantity > pkg.items[0].available);
});
