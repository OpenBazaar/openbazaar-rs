import assert from "node:assert/strict";
import { test } from "node:test";

import {
  afterShopPair,
  browserTicketPath,
  fetchOrders,
  mintBrowserPairingCode,
  onShopAwaitingCode,
  pairShop,
  placeCheckout,
  shopAwaitingCode,
  showPairingWith,
  ShopPairingNeeded,
} from "./catalog.ts";

const TOKEN = "ab".repeat(32);
const TICKET = "cd".repeat(32);
const MINTED = "WXYZ-2345";
const WRONG = "That pairing code is wrong or has expired.";
const ASK = "Type the pairing code from your node.";
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
const pairBodies: unknown[] = [];
const sessions: string[] = [];

if (typeof localStorage === "undefined") {
  const store = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => store.set(key, value),
      removeItem: (key: string) => store.delete(key),
    },
  });
}

/// A node that prints a code for `{}`, trades GOOD-CODE for the token, and serves paired routes.
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const path = new URL(String(input)).pathname;
  if (path === "/pair") {
    const body = JSON.parse(String(init?.body)) as { code?: string };
    pairBodies.push(body);
    const { code } = body;
    const header = new Headers(init?.headers).get("X-Ob-Pair");
    if (code === undefined) {
      if (header === TOKEN) return json(200, { code: MINTED });
      return json(401, { error: ASK, codeRequired: true });
    }
    return code === "GOOD-CODE" ? json(200, { token: TOKEN }) : json(401, { error: WRONG });
  }
  if (path === "/browser-session" && init?.method === "POST") {
    sessions.push(path);
    return new Response(`http://127.0.0.1:8787/browser/${TICKET}\n`);
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

test("Settings mint returns a code and not the pairing token", async () => {
  localStorage.setItem("ob-pair:http://127.0.0.1:8787", TOKEN);
  const code = await mintBrowserPairingCode();
  assert.equal(code, MINTED);
  assert.notEqual(code, TOKEN);
  const last = pairBodies.at(-1);
  assert.deepEqual(last, {});
});

test("the one-time path keeps the page's origin and is not the pairing token", () => {
  const path = browserTicketPath(`http://127.0.0.1:8787/browser/${TICKET}\n`, "http://localhost:8787");
  assert.equal(path, `http://localhost:8787/browser/${TICKET}`);
  assert.equal(browserTicketPath(`http://127.0.0.1:8787/browser/${TOKEN}`, "http://127.0.0.1:8787"), `http://127.0.0.1:8787/browser/${TOKEN}`);
  assert.equal(browserTicketPath("nope", "http://127.0.0.1:8787"), null);
});

function mockLocalShop(host: string) {
  const assigned = { url: "" };
  const previous = { document: (globalThis as { document?: unknown }).document, location: (globalThis as { location?: unknown }).location };
  (globalThis as { document?: unknown }).document = {
    querySelector: (sel: string) => (String(sel).includes("ob-local-shop") ? {} : null),
  };
  (globalThis as { location?: { hostname: string; origin: string; assign: (url: string) => void } }).location = {
    hostname: host,
    origin: `http://${host}:8787`,
    assign(url: string) {
      assigned.url = url;
    },
  };
  return {
    assigned,
    restore() {
      (globalThis as { document?: unknown }).document = previous.document;
      (globalThis as { location?: unknown }).location = previous.location;
    },
  };
}

test("a local shop on this computer pairs without a typed code", async () => {
  const shop = mockLocalShop("127.0.0.1");
  const before = sessions.length;
  const beforePair = pairBodies.length;
  try {
    const forget = showPairingWith(async (trade) => {
      await trade("");
      return true;
    });
    await pairShop();
    forget();
    assert.equal(sessions.length, before + 1);
    assert.equal(pairBodies.length, beforePair, "loopback Continue does not ask the node to print a code");
    assert.equal(shop.assigned.url, `http://127.0.0.1:8787/browser/${TICKET}`);
  } finally {
    shop.restore();
  }
});

test("a local-shop page that is not on loopback still types a code", async () => {
  const shop = mockLocalShop("example.onion");
  const before = sessions.length;
  try {
    const forget = showPairingWith(async (trade) => {
      await trade("GOOD-CODE");
      return true;
    });
    await pairShop();
    forget();
    assert.equal(sessions.length, before, "a forwarded shop does not mint a loopback cookie");
  } finally {
    shop.restore();
  }
});
