import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { test } from "node:test";

registerHooks({
  resolve(specifier, context, next) {
    if (context.parentURL?.endsWith("/useOrderMessageAlerts.ts")) {
      if (specifier === "react") return { url: "stub:react", shortCircuit: true };
      if (specifier === "../catalog") return { url: "stub:catalog", shortCircuit: true };
      if (specifier === "../seller") return { url: "stub:seller", shortCircuit: true };
      if (specifier === "../orderList") return { url: "stub:orderList", shortCircuit: true };
      if (specifier === "../useNodeResource") return { url: "stub:useNodeResource", shortCircuit: true };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "stub:react") {
      return { format: "module", source: "export const useCallback = f => f; export const useEffect = () => {}; export const useRef = v => ({ current: v }); export const useState = v => [v, () => {}];", shortCircuit: true };
    }
    if (url === "stub:catalog") {
      return { format: "module", source: "export const fetchOrders = async () => [];", shortCircuit: true };
    }
    if (url === "stub:seller") {
      return { format: "module", source: "export const fetchSales = async () => [];", shortCircuit: true };
    }
    if (url === "stub:orderList") {
      return { format: "module", source: "export const sameOrderSnapshot = (a, b) => JSON.stringify(a) === JSON.stringify(b);", shortCircuit: true };
    }
    if (url === "stub:useNodeResource") {
      return { format: "module", source: "export const useNodeResource = () => ({ data: null, error: '', loading: false, refresh: async () => {}, update: () => {} });", shortCircuit: true };
    }
    return next(url, context);
  },
});

const {
  combineSalesBadge,
  formatMessageAlert,
  incomingOrderMessages,
  incomingSaleMessages,
  markOrderMessagesSeen,
  parseSeenMessages,
  readSeenMessages,
  reconcileSideMessages,
  seenMessagesKey,
  toastNavigationTarget,
  writeSeenMessages,
} = await import("./useOrderMessageAlerts.ts");

test("incomingOrderMessages only includes messages from the seller", () => {
  const incoming = incomingOrderMessages([
    {
      id: "ord-1",
      seller: "Pine Goods",
      remote: {
        messages: [
          { id: "m1", from: "buyer", text: "Hello?", at: 10 },
          { id: "m2", from: "seller", text: "Shipped today", at: 20 },
        ],
      },
    },
    {
      id: "ord-2",
      seller: "  ",
      remote: {
        messages: [{ id: "m3", from: "seller", text: "Ready", at: 30 }],
      },
    },
    {
      id: "ord-3",
      seller: "No remote",
      remote: null,
    },
  ] as never);

  assert.deepEqual(incoming, [
    { key: "ord-1:m2", orderId: "ord-1", counterparty: "Pine Goods", at: 20 },
    { key: "ord-2:m3", orderId: "ord-2", counterparty: "seller", at: 30 },
  ]);
});

test("incomingSaleMessages only includes messages from the buyer", () => {
  const incoming = incomingSaleMessages([
    {
      id: "sale-1",
      messages: [
        { id: "s1", from: "seller", text: "Thanks!", at: 10 },
        { id: "s2", from: "buyer", text: "Where is tracking?", at: 20 },
      ],
    },
  ] as never);

  assert.deepEqual(incoming, [
    { key: "sale-1:s2", orderId: "sale-1", counterparty: "buyer", at: 20 },
  ]);
});

test("reconcileSideMessages seeds baseline on first load without firing toasts", () => {
  const incoming = [
    { key: "ord-1:m1", orderId: "ord-1", counterparty: "Pine Goods", at: 10 },
  ];
  const seeded = reconcileSideMessages({
    incoming,
    persistedSeen: null,
    sessionNotified: new Set(),
    initialized: false,
    activeOrderId: null,
  });
  assert.deepEqual(seeded.nextSeen, ["ord-1:m1"]);
  assert.equal(seeded.seenChanged, true);
  assert.equal(seeded.unreadCount, 0);
  assert.deepEqual(seeded.unreadByOrder, {});
  assert.deepEqual(seeded.freshToToast, []);
});

test("reconcileSideMessages detects new messages live, tracks per-order unread counts, suppresses duplicates, and avoids stale toasts on refresh", () => {
  const m1 = { key: "ord-1:m1", orderId: "ord-1", counterparty: "Pine Goods", at: 10 };
  const m2 = { key: "ord-1:m2", orderId: "ord-1", counterparty: "Pine Goods", at: 20 };
  const m3 = { key: "ord-2:m3", orderId: "ord-2", counterparty: "Mario Pokemon", at: 30 };

  // New messages arrive live while not viewing those order detail pages (including when viewing the list)
  const live = reconcileSideMessages({
    incoming: [m1, m2, m3],
    persistedSeen: ["ord-1:m1"],
    sessionNotified: new Set(["ord-1:m1"]),
    initialized: true,
    activeOrderId: null,
  });
  assert.equal(live.unreadCount, 2);
  assert.deepEqual(live.unreadByOrder, { "ord-1": 1, "ord-2": 1 });
  assert.equal(live.seenChanged, false);
  assert.deepEqual(live.freshToToast, [m2, m3]);

  // Next poll with same snapshot keeps unread=2 without re-toasting
  const repeat = reconcileSideMessages({
    incoming: [m1, m2, m3],
    persistedSeen: ["ord-1:m1"],
    sessionNotified: new Set(["ord-1:m1", "ord-1:m2", "ord-2:m3"]),
    initialized: true,
    activeOrderId: null,
  });
  assert.equal(repeat.unreadCount, 2);
  assert.deepEqual(repeat.unreadByOrder, { "ord-1": 1, "ord-2": 1 });
  assert.deepEqual(repeat.freshToToast, []);

  // Page refresh before visiting orders keeps unread=2 without firing stale toast
  const refreshed = reconcileSideMessages({
    incoming: [m1, m2, m3],
    persistedSeen: ["ord-1:m1"],
    sessionNotified: new Set(),
    initialized: false,
    activeOrderId: null,
  });
  assert.equal(refreshed.unreadCount, 2);
  assert.deepEqual(refreshed.unreadByOrder, { "ord-1": 1, "ord-2": 1 });
  assert.deepEqual(refreshed.freshToToast, []);
});

test("reconcileSideMessages marks only the active order seen and never drops previously seen keys", () => {
  const s1 = { key: "sale-1:s1", orderId: "sale-1", counterparty: "buyer", at: 10 };
  const s2 = { key: "sale-1:s2", orderId: "sale-1", counterparty: "buyer", at: 20 };
  const s3 = { key: "sale-2:s3", orderId: "sale-2", counterparty: "buyer", at: 30 };

  const whileViewingSale1 = reconcileSideMessages({
    incoming: [s1, s2, s3],
    persistedSeen: ["sale-1:s1"],
    sessionNotified: new Set(["sale-1:s1"]),
    initialized: true,
    activeOrderId: "sale-1",
  });
  assert.equal(whileViewingSale1.unreadCount, 1);
  assert.deepEqual(whileViewingSale1.unreadByOrder, { "sale-2": 1 });
  assert.equal(whileViewingSale1.seenChanged, true);
  assert.deepEqual(whileViewingSale1.nextSeen, ["sale-1:s1", "sale-1:s2"]);
  assert.deepEqual(whileViewingSale1.freshToToast, [s2, s3]);

  // Stale snapshot missing s2 does not erase s2 from persistedSeen
  const stale = reconcileSideMessages({
    incoming: [s1],
    persistedSeen: ["sale-1:s1", "sale-1:s2"],
    sessionNotified: new Set(["sale-1:s1", "sale-1:s2"]),
    initialized: true,
    activeOrderId: "sale-1",
  });
  assert.equal(stale.seenChanged, false);
  assert.deepEqual(stale.nextSeen, ["sale-1:s1", "sale-1:s2"]);
  assert.deepEqual(stale.unreadByOrder, {});
});

test("markOrderMessagesSeen clears unread only for the opened order", () => {
  const incoming = [
    { key: "ord-1:m1", orderId: "ord-1", counterparty: "Pine Goods", at: 10 },
    { key: "ord-1:m2", orderId: "ord-1", counterparty: "Pine Goods", at: 20 },
    { key: "ord-2:m3", orderId: "ord-2", counterparty: "Mario Pokemon", at: 30 },
  ];
  const updated = markOrderMessagesSeen({
    incoming,
    persistedSeen: ["ord-1:m1"],
    orderId: "ord-1",
  });
  assert.equal(updated.seenChanged, true);
  assert.deepEqual(updated.nextSeen, ["ord-1:m1", "ord-1:m2"]);
  assert.deepEqual(updated.unreadByOrder, { "ord-2": 1 });
  assert.equal(updated.unreadCount, 1);
});

test("toastNavigationTarget returns the orderId only when all fresh messages belong to one order", () => {
  assert.equal(toastNavigationTarget([]), undefined);
  assert.equal(
    toastNavigationTarget([{ key: "ord-1:m1", orderId: "ord-1", counterparty: "Mario Pokemon", at: 1 }]),
    "ord-1",
  );
  assert.equal(
    toastNavigationTarget([
      { key: "ord-1:m1", orderId: "ord-1", counterparty: "Mario Pokemon", at: 1 },
      { key: "ord-1:m2", orderId: "ord-1", counterparty: "Mario Pokemon", at: 2 },
    ]),
    "ord-1",
  );
  assert.equal(
    toastNavigationTarget([
      { key: "ord-1:m1", orderId: "ord-1", counterparty: "Mario Pokemon", at: 1 },
      { key: "ord-2:m2", orderId: "ord-2", counterparty: "Pine Goods", at: 2 },
    ]),
    undefined,
  );
});

test("formatMessageAlert formats single and multi-message notices", () => {
  assert.equal(formatMessageAlert("orders", []), null);
  assert.equal(
    formatMessageAlert("orders", [{ key: "1:a", orderId: "1", counterparty: "Pine Goods", at: 1 }]),
    "New message from Pine Goods",
  );
  assert.equal(
    formatMessageAlert("orders", [
      { key: "1:a", orderId: "1", counterparty: "Pine Goods", at: 1 },
      { key: "1:b", orderId: "1", counterparty: "Pine Goods", at: 2 },
    ]),
    "2 new messages from Pine Goods",
  );
  assert.equal(
    formatMessageAlert("orders", [
      { key: "1:a", orderId: "1", counterparty: "Pine Goods", at: 1 },
      { key: "2:b", orderId: "2", counterparty: "Oak Craft", at: 2 },
    ]),
    "2 new order messages",
  );
  assert.equal(
    formatMessageAlert("sales", [{ key: "s1:a", orderId: "s1", counterparty: "buyer", at: 1 }]),
    "New message from buyer",
  );
  assert.equal(
    formatMessageAlert("sales", [
      { key: "s1:a", orderId: "s1", counterparty: "buyer", at: 1 },
      { key: "s2:b", orderId: "s2", counterparty: "buyer", at: 2 },
    ]),
    "2 new messages from buyers",
  );
});

test("combineSalesBadge merges unread sale messages with existing salesBadge state", () => {
  const neutral = { count: 5, accent: false, ariaLabel: "5 sales" };
  const accented = { count: 1, accent: true, ariaLabel: "5 sales, 1 new" };

  assert.deepEqual(combineSalesBadge(neutral, 0), neutral);
  assert.deepEqual(combineSalesBadge(null, 2), {
    count: 2,
    accent: true,
    ariaLabel: "2 unread messages",
  });
  assert.deepEqual(combineSalesBadge(neutral, 1), {
    count: 1,
    accent: true,
    ariaLabel: "5 sales, 1 unread message",
  });
  assert.deepEqual(combineSalesBadge(accented, 2), {
    count: 3,
    accent: true,
    ariaLabel: "5 sales, 1 new, 2 unread messages",
  });
});

test("seen message persistence round-trips per storeKey and handles invalid JSON", () => {
  const mem: Record<string, string> = {};
  const storage = {
    getItem: (key: string) => mem[key] ?? null,
    setItem: (key: string, value: string) => { mem[key] = value; },
  };
  writeSeenMessages("store-a", { orders: ["o1:m1"], sales: ["s1:m2"] }, storage);
  assert.equal(seenMessagesKey("store-a"), "ob-order-messages-seen:store-a");
  assert.deepEqual(readSeenMessages("store-a", storage), { orders: ["o1:m1"], sales: ["s1:m2"] });
  assert.deepEqual(readSeenMessages("store-b", storage), { orders: null, sales: null });
  assert.deepEqual(parseSeenMessages("not-json"), { orders: null, sales: null });
  assert.deepEqual(parseSeenMessages(JSON.stringify({ orders: "bad", sales: [1, "ok"] })), {
    orders: null,
    sales: ["ok"],
  });
});
