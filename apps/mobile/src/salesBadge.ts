// Sidebar Sales badge: total when the seller has seen the list, new-count
// when a sale arrived after they last opened Sales or moved into a status
// that needs them. Last seen is kept on this device, per store.

export type SaleSight = { id: string; created: number; status: string };

export type SalesSeen = {
  openedAt: number;
  statuses: Record<string, string>;
};

export type SalesBadge = {
  count: number;
  accent: boolean;
  ariaLabel: string;
};

/** Statuses that need the seller: take the order, or fulfill a paid one. */
export const SALES_NEEDING_ACTION = ["Awaiting seller", "Paid"] as const;

type StorageLike = { getItem(key: string): string | null; setItem(key: string, value: string): void };

export function salesSeenKey(storeKey: string): string {
  return `ob-sales-seen:${storeKey}`;
}

export function saleNeedsAction(status: string): boolean {
  return status === "Awaiting seller" || status === "Paid";
}

export function salesSeenSnapshot(sales: SaleSight[], openedAt = Math.floor(Date.now() / 1000)): SalesSeen {
  const statuses: Record<string, string> = {};
  for (const sale of sales) statuses[sale.id] = sale.status;
  return { openedAt, statuses };
}

export function parseSalesSeen(raw: string | null): SalesSeen | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as { openedAt?: unknown; statuses?: unknown };
    if (typeof value.openedAt !== "number" || !Number.isFinite(value.openedAt) || value.openedAt < 0) return null;
    if (!value.statuses || typeof value.statuses !== "object" || Array.isArray(value.statuses)) return null;
    const statuses: Record<string, string> = {};
    for (const [id, status] of Object.entries(value.statuses)) {
      if (typeof status === "string") statuses[id] = status;
    }
    return { openedAt: value.openedAt, statuses };
  } catch {
    return null;
  }
}

function shopStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readSalesSeen(storeKey: string, storage: StorageLike | null = shopStorage()): SalesSeen | null {
  if (!storeKey || !storage) return null;
  try {
    return parseSalesSeen(storage.getItem(salesSeenKey(storeKey)));
  } catch {
    return null;
  }
}

export function writeSalesSeen(storeKey: string, seen: SalesSeen, storage: StorageLike | null = shopStorage()): void {
  if (!storeKey || !storage) return;
  try {
    storage.setItem(salesSeenKey(storeKey), JSON.stringify(seen));
  } catch {
    // The page still keeps last seen for this visit.
  }
}

/** New if it was not on the list when Sales was last opened, or it moved into a status that needs the seller. */
export function saleIsNew(sale: SaleSight, seen: SalesSeen | null): boolean {
  if (!seen) return true;
  const previous = seen.statuses[sale.id];
  if (previous === undefined) return true;
  return saleNeedsAction(sale.status) && previous !== sale.status;
}

function salesWords(total: number): string {
  return total === 1 ? "1 sale" : `${total} sales`;
}

export function salesBadge(sales: SaleSight[], seen: SalesSeen | null): SalesBadge | null {
  const total = sales.length;
  if (total === 0) return null;
  const newCount = sales.filter(sale => saleIsNew(sale, seen)).length;
  if (newCount > 0) return { count: newCount, accent: true, ariaLabel: `${salesWords(total)}, ${newCount} new` };
  return { count: total, accent: false, ariaLabel: salesWords(total) };
}
