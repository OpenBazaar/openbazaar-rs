import { useCallback, useEffect, useRef, useState } from "react";
import { fetchOrders, type BuyerOrder } from "../catalog";
import type { OrderMessage } from "../messages";
import { sameOrderSnapshot } from "../orderList";
import type { SalesBadge } from "../salesBadge";
import { fetchSales, type Sale } from "../seller";
import { useNodeResource } from "../useNodeResource";

export type OrderMessageAlertSide = "orders" | "sales";

export type OrderMessageToast = {
  message: string;
  action?: { label: string; onClick: () => void };
};

export type IncomingMessageRef = {
  key: string;
  orderId: string;
  counterparty: string;
  at: number;
};

export type SeenMessageStore = {
  orders: string[] | null;
  sales: string[] | null;
};

export type ReconcileResult = {
  nextSeen: string[];
  seenChanged: boolean;
  unreadCount: number;
  unreadByOrder: Record<string, number>;
  freshToToast: IncomingMessageRef[];
};

export type UseOrderMessageAlertsOptions = {
  nodeAvailable: boolean;
  seller: boolean;
  storeKey?: string;
  viewingOrders: boolean;
  viewingSales: boolean;
  /** The specific buyer order currently open on its dedicated detail page, if any. */
  activeOrderId?: string | null;
  /** The specific seller sale currently open on its dedicated detail page, if any. */
  activeSaleId?: string | null;
  /** Optional live sales snapshot (e.g. from useSales) to avoid duplicate /sales requests while viewing Sales. */
  salesData?: Sale[] | null;
  onAlert: (toast: OrderMessageToast) => void;
  onNavigate: (side: OrderMessageAlertSide, orderId?: string) => void;
  intervalMs?: number;
};

export type OrderMessageAlerts = {
  unreadOrders: number;
  unreadSales: number;
  unreadOrdersById: Record<string, number>;
  unreadSalesById: Record<string, number>;
  markOrderSeen: (side: OrderMessageAlertSide, orderId: string) => void;
  markSeen: (side: OrderMessageAlertSide) => void;
};

type StorageLike = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
};

const DEFAULT_POLL_MS = 12_000;
const MAX_SEEN_KEYS = 2_000;

export function seenMessagesKey(storeKey?: string): string {
  return storeKey ? `ob-order-messages-seen:${storeKey}` : "ob-order-messages-seen";
}

export function messageKey(orderId: string, message: OrderMessage): string {
  const id = message.id || `${message.at}:${message.text}`;
  return `${orderId}:${id}`;
}

/** Incoming messages on buyer orders are those sent by the seller (ignore buyer's own messages). */
export function incomingOrderMessages(orders: readonly BuyerOrder[]): IncomingMessageRef[] {
  const incoming: IncomingMessageRef[] = [];
  for (const order of orders) {
    const messages = order.remote?.messages;
    if (!messages?.length) continue;
    const counterparty = order.seller.trim() || "seller";
    for (const msg of messages) {
      if (msg.from !== "seller") continue;
      incoming.push({ key: messageKey(order.id, msg), orderId: order.id, counterparty, at: msg.at });
    }
  }
  return incoming;
}

/** Incoming messages on seller sales are those sent by the buyer (ignore seller's own messages). */
export function incomingSaleMessages(sales: readonly Sale[]): IncomingMessageRef[] {
  const incoming: IncomingMessageRef[] = [];
  for (const sale of sales) {
    if (!sale.messages.length) continue;
    for (const msg of sale.messages) {
      if (msg.from !== "buyer") continue;
      incoming.push({ key: messageKey(sale.id, msg), orderId: sale.id, counterparty: "buyer", at: msg.at });
    }
  }
  return incoming;
}

export function formatMessageAlert(
  side: OrderMessageAlertSide,
  fresh: readonly IncomingMessageRef[],
): string | null {
  if (fresh.length === 0) return null;
  if (fresh.length === 1) {
    return `New message from ${fresh[0].counterparty}`;
  }
  if (side === "sales") {
    const uniqueSales = new Set(fresh.map(item => item.orderId));
    return uniqueSales.size === 1
      ? `${fresh.length} new messages from buyer`
      : `${fresh.length} new messages from buyers`;
  }
  const names = new Set(fresh.map(item => item.counterparty));
  if (names.size === 1) {
    const [only] = names;
    return `${fresh.length} new messages from ${only}`;
  }
  return `${fresh.length} new order messages`;
}

export function combineSalesBadge(base: SalesBadge | null, unreadMessages: number): SalesBadge | null {
  if (unreadMessages <= 0) return base;
  const msgLabel = unreadMessages === 1 ? "1 unread message" : `${unreadMessages} unread messages`;
  if (!base) {
    return { count: unreadMessages, accent: true, ariaLabel: msgLabel };
  }
  if (base.accent) {
    return {
      count: base.count + unreadMessages,
      accent: true,
      ariaLabel: `${base.ariaLabel}, ${msgLabel}`,
    };
  }
  return {
    count: unreadMessages,
    accent: true,
    ariaLabel: `${base.ariaLabel}, ${msgLabel}`,
  };
}

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function parseKeyList(value: unknown): string[] | null {
  if (value === null || value === undefined) return null;
  if (!Array.isArray(value)) return null;
  return value.filter((item): item is string => typeof item === "string");
}

export function parseSeenMessages(raw: string | null): SeenMessageStore {
  if (!raw) return { orders: null, sales: null };
  try {
    const parsed = JSON.parse(raw) as { orders?: unknown; sales?: unknown };
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { orders: null, sales: null };
    }
    return {
      orders: parseKeyList(parsed.orders),
      sales: parseKeyList(parsed.sales),
    };
  } catch {
    return { orders: null, sales: null };
  }
}

export function readSeenMessages(
  storeKey?: string,
  storage: StorageLike | null = defaultStorage(),
): SeenMessageStore {
  if (!storage) return { orders: null, sales: null };
  try {
    return parseSeenMessages(storage.getItem(seenMessagesKey(storeKey)));
  } catch {
    return { orders: null, sales: null };
  }
}

export function writeSeenMessages(
  storeKey: string | undefined,
  seen: SeenMessageStore,
  storage: StorageLike | null = defaultStorage(),
): void {
  if (!storage) return;
  try {
    storage.setItem(seenMessagesKey(storeKey), JSON.stringify(seen));
  } catch {
    // Keep in-memory tracking even if storage is unavailable or full.
  }
}

export function toastNavigationTarget(fresh: readonly IncomingMessageRef[]): string | undefined {
  if (fresh.length === 0) return undefined;
  const uniqueOrderIds = new Set(fresh.map(item => item.orderId));
  return uniqueOrderIds.size === 1 ? fresh[0].orderId : undefined;
}

export function countUnreadByOrder(
  incoming: readonly IncomingMessageRef[],
  seenSet: ReadonlySet<string>,
): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const item of incoming) {
    if (seenSet.has(item.key)) continue;
    counts[item.orderId] = (counts[item.orderId] ?? 0) + 1;
  }
  return counts;
}

function sumUnread(counts: Record<string, number>): number {
  let total = 0;
  for (const count of Object.values(counts)) total += count;
  return total;
}

export function markOrderMessagesSeen(params: {
  incoming: readonly IncomingMessageRef[];
  persistedSeen: readonly string[] | null;
  orderId: string;
}): { nextSeen: string[]; seenChanged: boolean; unreadByOrder: Record<string, number>; unreadCount: number } {
  const { incoming, persistedSeen, orderId } = params;
  const existing = persistedSeen ?? [];
  const seenSet = new Set(existing);
  const orderKeys = incoming.filter(item => item.orderId === orderId).map(item => item.key);
  const hasNew = orderKeys.some(key => !seenSet.has(key));
  const nextSeen = hasNew
    ? Array.from(new Set([...existing, ...orderKeys])).slice(-MAX_SEEN_KEYS)
    : [...existing];
  const nextSeenSet = new Set(nextSeen);
  const unreadByOrder = countUnreadByOrder(incoming, nextSeenSet);
  return {
    nextSeen,
    seenChanged: hasNew,
    unreadByOrder,
    unreadCount: sumUnread(unreadByOrder),
  };
}

export function reconcileSideMessages(params: {
  incoming: readonly IncomingMessageRef[];
  persistedSeen: readonly string[] | null;
  sessionNotified: ReadonlySet<string>;
  initialized: boolean;
  activeOrderId?: string | null;
}): ReconcileResult {
  const { incoming, persistedSeen, sessionNotified, initialized, activeOrderId = null } = params;
  const allKeys = incoming.map(item => item.key);

  if (persistedSeen === null) {
    return {
      nextSeen: allKeys.slice(-MAX_SEEN_KEYS),
      seenChanged: true,
      unreadCount: 0,
      unreadByOrder: {},
      freshToToast: [],
    };
  }

  const seenSet = new Set(persistedSeen);
  const unseen = incoming.filter(item => !seenSet.has(item.key));
  const freshToToast = initialized
    ? unseen.filter(item => !sessionNotified.has(item.key))
    : [];

  if (activeOrderId) {
    const activeUnseenKeys = unseen
      .filter(item => item.orderId === activeOrderId)
      .map(item => item.key);
    if (activeUnseenKeys.length > 0) {
      const merged = Array.from(new Set([...persistedSeen, ...activeUnseenKeys])).slice(-MAX_SEEN_KEYS);
      const nextSeenSet = new Set(merged);
      const unreadByOrder = countUnreadByOrder(incoming, nextSeenSet);
      return {
        nextSeen: merged,
        seenChanged: true,
        unreadCount: sumUnread(unreadByOrder),
        unreadByOrder,
        freshToToast,
      };
    }
  }

  const unreadByOrder = countUnreadByOrder(incoming, seenSet);
  return {
    nextSeen: [...persistedSeen],
    seenChanged: false,
    unreadCount: sumUnread(unreadByOrder),
    unreadByOrder,
    freshToToast,
  };
}

/**
 * Gently polls buyer orders (`GET /orders`) and seller sales (`GET /sales`),
 * which both embed order messages inline, to fire real-time actionable toasts
 * and maintain per-order and sidebar unread counts for incoming messages.
 */
export function useOrderMessageAlerts({
  nodeAvailable,
  seller,
  storeKey,
  viewingOrders: _viewingOrders,
  viewingSales,
  activeOrderId = null,
  activeSaleId = null,
  salesData,
  onAlert,
  onNavigate,
  intervalMs = DEFAULT_POLL_MS,
}: UseOrderMessageAlertsOptions): OrderMessageAlerts {
  const ready = nodeAvailable && storeKey !== "";
  const [unreadOrders, setUnreadOrders] = useState(0);
  const [unreadSales, setUnreadSales] = useState(0);
  const [unreadOrdersById, setUnreadOrdersById] = useState<Record<string, number>>({});
  const [unreadSalesById, setUnreadSalesById] = useState<Record<string, number>>({});

  const seenRef = useRef<SeenMessageStore>({ orders: null, sales: null });
  const notifiedRef = useRef<{ orders: Set<string>; sales: Set<string> }>({
    orders: new Set(),
    sales: new Set(),
  });
  const initializedRef = useRef<{ orders: boolean; sales: boolean }>({
    orders: false,
    sales: false,
  });
  const latestIncomingRef = useRef<{
    orders: IncomingMessageRef[] | null;
    sales: IncomingMessageRef[] | null;
  }>({
    orders: null,
    sales: null,
  });

  const onAlertRef = useRef(onAlert);
  onAlertRef.current = onAlert;
  const onNavigateRef = useRef(onNavigate);
  onNavigateRef.current = onNavigate;
  const activeIdRef = useRef<{ orders: string | null; sales: string | null }>({
    orders: activeOrderId,
    sales: activeSaleId,
  });
  activeIdRef.current = { orders: activeOrderId, sales: activeSaleId };
  const storeKeyRef = useRef(storeKey);
  storeKeyRef.current = storeKey;

  useEffect(() => {
    seenRef.current = ready ? readSeenMessages(storeKey) : { orders: null, sales: null };
    notifiedRef.current = { orders: new Set(), sales: new Set() };
    initializedRef.current = { orders: false, sales: false };
    latestIncomingRef.current = { orders: null, sales: null };
    setUnreadOrders(0);
    setUnreadSales(0);
    setUnreadOrdersById({});
    setUnreadSalesById({});
  }, [ready, storeKey]);

  const ordersResource = useNodeResource(
    fetchOrders,
    intervalMs,
    ready,
    undefined,
    sameOrderSnapshot,
  );

  // While viewing Sales, `useSales` already polls `/sales` every 10s; avoid duplicate requests.
  const pollSalesDirectly = ready && seller && !(viewingSales && salesData !== undefined);
  const polledSalesResource = useNodeResource(
    fetchSales,
    intervalMs,
    pollSalesDirectly,
    undefined,
    sameOrderSnapshot,
  );

  const activeSales = viewingSales && salesData !== undefined ? salesData : polledSalesResource.data;

  const processSide = useCallback((side: OrderMessageAlertSide, incoming: IncomingMessageRef[]) => {
    latestIncomingRef.current[side] = incoming;
    const activeId = activeIdRef.current[side];
    const result = reconcileSideMessages({
      incoming,
      persistedSeen: seenRef.current[side],
      sessionNotified: notifiedRef.current[side],
      initialized: initializedRef.current[side],
      activeOrderId: activeId,
    });

    if (result.seenChanged) {
      seenRef.current = { ...seenRef.current, [side]: result.nextSeen };
      writeSeenMessages(storeKeyRef.current, seenRef.current);
    }

    for (const item of incoming) {
      notifiedRef.current[side].add(item.key);
    }
    initializedRef.current[side] = true;

    if (side === "orders") {
      setUnreadOrders(result.unreadCount);
      setUnreadOrdersById(result.unreadByOrder);
    } else {
      setUnreadSales(result.unreadCount);
      setUnreadSalesById(result.unreadByOrder);
    }

    const text = formatMessageAlert(side, result.freshToToast);
    if (text) {
      const targetOrderId = toastNavigationTarget(result.freshToToast);
      const alreadyViewingTarget = targetOrderId !== undefined && activeId === targetOrderId;
      onAlertRef.current(
        alreadyViewingTarget
          ? { message: text }
          : {
              message: text,
              action: {
                label: targetOrderId
                  ? (side === "orders" ? "View order" : "View sale")
                  : (side === "orders" ? "View orders" : "View sales"),
                onClick: () => onNavigateRef.current(side, targetOrderId),
              },
            },
      );
    }
  }, []);

  useEffect(() => {
    if (!ready || !ordersResource.data) return;
    processSide("orders", incomingOrderMessages(ordersResource.data));
  }, [ready, ordersResource.data, processSide]);

  useEffect(() => {
    if (!ready || !seller || !activeSales) return;
    processSide("sales", incomingSaleMessages(activeSales));
  }, [ready, seller, activeSales, processSide]);

  const markOrderSeen = useCallback((side: OrderMessageAlertSide, orderId: string) => {
    const incoming = latestIncomingRef.current[side];
    if (!incoming) return;
    for (const item of incoming) {
      if (item.orderId === orderId) notifiedRef.current[side].add(item.key);
    }
    const updated = markOrderMessagesSeen({
      incoming,
      persistedSeen: seenRef.current[side],
      orderId,
    });
    if (updated.seenChanged) {
      seenRef.current = { ...seenRef.current, [side]: updated.nextSeen };
      writeSeenMessages(storeKeyRef.current, seenRef.current);
    }
    if (side === "orders") {
      setUnreadOrders(updated.unreadCount);
      setUnreadOrdersById(updated.unreadByOrder);
    } else {
      setUnreadSales(updated.unreadCount);
      setUnreadSalesById(updated.unreadByOrder);
    }
  }, []);

  const markSeen = useCallback((side: OrderMessageAlertSide) => {
    const incoming = latestIncomingRef.current[side];
    if (incoming) {
      const keys = incoming.map(item => item.key);
      for (const key of keys) notifiedRef.current[side].add(key);
      const merged = Array.from(new Set([...(seenRef.current[side] ?? []), ...keys])).slice(-MAX_SEEN_KEYS);
      seenRef.current = { ...seenRef.current, [side]: merged };
      writeSeenMessages(storeKeyRef.current, seenRef.current);
    }
    if (side === "orders") {
      setUnreadOrders(0);
      setUnreadOrdersById({});
    } else {
      setUnreadSales(0);
      setUnreadSalesById({});
    }
  }, []);

  return { unreadOrders, unreadSales, unreadOrdersById, unreadSalesById, markOrderSeen, markSeen };
}
