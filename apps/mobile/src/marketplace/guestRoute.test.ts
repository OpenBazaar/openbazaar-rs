import assert from "node:assert/strict";
import { test } from "node:test";

import { sanitizeGuestLocation } from "./guestRoute.ts";
import type { ShopLocation } from "./shopHistory.ts";

test("guest mode redirects node-only sections and receipts cleanly", () => {
  const wallet: ShopLocation = { section: "wallet", category: null, page: { name: "browse" } };
  assert.deepEqual(sanitizeGuestLocation(wallet, false), wallet);
  assert.deepEqual(sanitizeGuestLocation(wallet, true), { section: "discover", category: null, page: { name: "browse" } });

  for (const section of ["orders", "addresses", "home-node", "browsers", "lightning", "zcash", "monero", "tor", "follows", "store-labels"]) {
    assert.deepEqual(
      sanitizeGuestLocation({ section, category: null, page: { name: "browse" } }, true),
      { section: "discover", category: null, page: { name: "browse" } },
      section,
    );
  }

  for (const section of ["storefront", "listings", "coupons", "sales"]) {
    assert.deepEqual(
      sanitizeGuestLocation({ section, category: null, page: { name: "browse" } }, true),
      { section: "store", category: null, page: { name: "browse" } },
      section,
    );
  }

  for (const section of ["discover", "saved", "bag", "safety", "store"]) {
    const loc: ShopLocation = { section, category: section === "discover" ? "Books" : null, page: { name: "browse" } };
    assert.deepEqual(sanitizeGuestLocation(loc, true), loc, section);
  }

  const receipt = {
    section: "orders",
    category: null,
    page: { name: "receipt", receipt: { checkoutId: "c1", itemsSats: 1, shippingSats: 0, totalSats: 1, orders: [] } },
  } as ShopLocation;
  assert.deepEqual(sanitizeGuestLocation(receipt, true), { section: "discover", category: null, page: { name: "browse" } });

  const orderDetail: ShopLocation = { section: "orders", category: null, page: { name: "order", orderId: "ord-1" } };
  assert.deepEqual(sanitizeGuestLocation(orderDetail, false), orderDetail);
  assert.deepEqual(sanitizeGuestLocation(orderDetail, true), { section: "discover", category: null, page: { name: "browse" } });

  const saleDetail: ShopLocation = { section: "sales", category: null, page: { name: "sale", saleId: "sale-1" } };
  assert.deepEqual(sanitizeGuestLocation(saleDetail, false), saleDetail);
  assert.deepEqual(sanitizeGuestLocation(saleDetail, true), { section: "store", category: null, page: { name: "browse" } });
});
