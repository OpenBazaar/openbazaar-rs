import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import {
  parseSalesSeen,
  readSalesSeen,
  saleIsNew,
  saleNeedsAction,
  salesBadge,
  salesSeenKey,
  salesSeenSnapshot,
  SALES_NEEDING_ACTION,
  writeSalesSeen,
  type SaleSight,
  type SalesSeen,
} from "./salesBadge.ts";

function sale(id: string, status: string, created = 50): SaleSight {
  return { id, created, status };
}

function seen(statuses: Record<string, string>, openedAt = 100): SalesSeen {
  return { openedAt, statuses };
}

test("the badge is hidden when there are no sales", () => {
  assert.equal(salesBadge([], null), null);
  assert.equal(salesBadge([], seen({})), null);
});

test("unseen sales use the new count and an accent badge", () => {
  const sales = [sale("a", "Awaiting payment"), sale("b", "Paid"), sale("c", "Shipped")];
  assert.deepEqual(salesBadge(sales, null), { count: 3, accent: true, ariaLabel: "3 sales, 3 new" });
  assert.deepEqual(salesBadge([sale("a", "Paid")], null), { count: 1, accent: true, ariaLabel: "1 sale, 1 new" });
});

test("seen sales with no new ones show the total in the neutral badge", () => {
  const sales = [sale("a", "Awaiting payment", 10), sale("b", "Shipped", 20)];
  const last = seen({ a: "Awaiting payment", b: "Shipped" }, 100);
  assert.deepEqual(salesBadge(sales, last), { count: 2, accent: false, ariaLabel: "2 sales" });
});

test("a sale that arrived after Sales was last opened is new", () => {
  const last = seen({ a: "Awaiting payment" }, 100);
  assert.equal(saleIsNew(sale("a", "Awaiting payment", 40), last), false);
  assert.equal(saleIsNew(sale("a", "Awaiting payment", 140), last), false);
  assert.equal(saleIsNew(sale("b", "Awaiting payment", 140), last), true);
  assert.deepEqual(salesBadge([sale("a", "Awaiting payment", 40), sale("b", "Awaiting payment", 140)], last), {
    count: 1,
    accent: true,
    ariaLabel: "2 sales, 1 new",
  });
});

test("Paid and Awaiting seller need the seller; other statuses do not", () => {
  assert.deepEqual([...SALES_NEEDING_ACTION], ["Awaiting seller", "Paid"]);
  assert.equal(saleNeedsAction("Awaiting seller"), true);
  assert.equal(saleNeedsAction("Paid"), true);
  for (const status of ["Awaiting payment", "Payment seen", "Shipped", "Completed"]) {
    assert.equal(saleNeedsAction(status), false, status);
  }
});

test("a seen sale that changes into a status that needs the seller is new", () => {
  const last = seen({ a: "Awaiting payment", b: "Awaiting payment", c: "Paid" }, 100);
  assert.equal(saleIsNew(sale("a", "Paid", 40), last), true);
  assert.equal(saleIsNew(sale("b", "Awaiting seller", 40), last), true);
  assert.equal(saleIsNew(sale("c", "Paid", 40), last), false);
  assert.equal(saleIsNew(sale("a", "Payment seen", 40), last), false);
  assert.equal(saleIsNew(sale("a", "Shipped", 40), last), false);
  assert.deepEqual(salesBadge([sale("a", "Paid", 40), sale("c", "Paid", 40)], last), {
    count: 1,
    accent: true,
    ariaLabel: "2 sales, 1 new",
  });
});

test("opening Sales snapshots the current list so the badge returns to the total", () => {
  const sales = [sale("a", "Paid", 10), sale("b", "Awaiting seller", 20)];
  assert.equal(salesBadge(sales, null)?.accent, true);
  const opened = salesSeenSnapshot(sales, 200);
  assert.deepEqual(opened.statuses, { a: "Paid", b: "Awaiting seller" });
  assert.deepEqual(salesBadge(sales, opened), { count: 2, accent: false, ariaLabel: "2 sales" });
});

test("last seen is stored per store key", () => {
  const mem: Record<string, string> = {};
  const storage = { getItem: (key: string) => mem[key] ?? null, setItem: (key: string, value: string) => { mem[key] = value; } };
  const pine = salesSeenSnapshot([sale("a", "Paid")], 10);
  const oak = salesSeenSnapshot([sale("b", "Awaiting payment")], 11);
  writeSalesSeen("aa".repeat(32), pine, storage);
  writeSalesSeen("bb".repeat(32), oak, storage);
  assert.deepEqual(Object.keys(mem).sort(), [salesSeenKey("aa".repeat(32)), salesSeenKey("bb".repeat(32))].sort());
  assert.match(salesSeenKey("aa".repeat(32)), /^ob-sales-seen:[0-9a-f]{64}$/);
  assert.deepEqual(readSalesSeen("aa".repeat(32), storage), pine);
  assert.deepEqual(readSalesSeen("bb".repeat(32), storage), oak);
  assert.equal(readSalesSeen("", storage), null);
  assert.equal(parseSalesSeen("not json"), null);
  assert.equal(parseSalesSeen(JSON.stringify({ openedAt: 1, statuses: [] })), null);
});

test("the shop Sales link uses the badge and marks the list seen on the Sales page", () => {
  const shop = readFileSync(new URL("./marketplace/Marketplace.tsx", import.meta.url), "utf8");
  const sales = readFileSync(new URL("./marketplace/Sales.tsx", import.meta.url), "utf8");
  const css = readFileSync(new URL("../public/marketplace.css", import.meta.url), "utf8");
  assert.match(shop, /salesBadge\(/);
  assert.match(shop, /countNew=\{salesCount\?\.accent\}/);
  assert.match(shop, /countLabel=\{salesCount\?\.ariaLabel\}/);
  assert.doesNotMatch(shop, /salesToShip/);
  assert.match(sales, /seen\.current\?\.\(sales\.data\)/);
  assert.match(css, /\.ob-main-nav \.ob-nav-count\.is-new/);
  assert.match(css, /background: var\(--ob-action\)/);
});
