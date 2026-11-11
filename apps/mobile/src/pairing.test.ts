import assert from "node:assert/strict";
import { test } from "node:test";

import { afterShopPair, fetchOrders, onShopAwaitingCode, pairShop, placeCheckout, shopAwaitingCode, showPairingWith, ShopPairingNeeded } from "./catalog.ts";

const TOKEN = "ab".repeat(32);
const WRONG = "That pairing code is wrong or has expired.";
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });

/// A node that prints a code for `{}`, trades GOOD-CODE for the token, and serves paired routes.
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const path = new URL(String(input)).pathname;
  if (path === "/pair") {
    const { code } = JSON.parse(String(init?.body)) as { code?: string };
    if (code === undefined) return json(401, { error: "Type the pairing code shown in your node's terminal.", codeRequired: true });
    return code === "GOOD-CODE" ? json(200, { token: TOKEN }) : json(401, { error: WRONG });
  }
  if (new Headers(init?.headers).get("X-Ob-Pair") !== TOKEN) return json(403, { error: "The shop is not paired with this node." });
  return json(200, { orders: [] });
}) as typeof fetch;

// The built-in browser throws here. Pairing must not depend on it.
(globalThis as { prompt?: unknown }).prompt = () => {
  throw new Error("prompt() is not supported.");
};

test("a page without the pairing dialog says so instead of calling prompt()", async () => {
  await assert.rejects(fetchOrders(), /Open the shop in a browser/);
});

test("loading a page never opens the dialog; a click does, and closing it leaves the banner", async () => {
  let asked = 0;
  const forget = showPairingWith(async () => {
    asked += 1;
    return false;
  });
  const seen: boolean[] = [];
  const unwatch = onShopAwaitingCode(() => seen.push(shopAwaitingCode()));
  await assert.rejects(fetchOrders(), ShopPairingNeeded);
  assert.equal(asked, 0, "a page load shows the banner instead");
  await assert.rejects(placeCheckout({ packages: [] }), ShopPairingNeeded);
  assert.equal(asked, 1, "Place order opens it");
  await assert.rejects(fetchOrders(), ShopPairingNeeded);
  assert.equal(asked, 1, "a poll after Not now does not reopen it");
  assert.deepEqual(seen, [true, false, true], "the banner hides while the dialog is open");
  unwatch();
  forget();
});

test("requests share one dialog, and a wrong code can be typed again", async () => {
  let asked = 0;
  const forget = showPairingWith(async (trade) => {
    asked += 1;
    await assert.rejects(trade("NOPE-NOPE"), { message: WRONG });
    await trade("GOOD-CODE");
    return true;
  });
  let paired = 0;
  const unwatch = afterShopPair(() => (paired += 1));
  const [, first, second] = await Promise.all([pairShop(), fetchOrders(), fetchOrders()]);
  assert.deepEqual([first, second], [[], []]);
  assert.equal(asked, 1);
  assert.equal(paired, 1, "pages are told to load again");
  assert.equal(shopAwaitingCode(), false);
  unwatch();
  forget();
});
