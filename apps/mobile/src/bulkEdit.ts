import type { Availability } from "./commerce.ts";
import { toMinor } from "./pricing.ts";

/** Where a bulk edit is signed. Drafts are not published from here. */
export const BULK_PATH = "/store/listings/bulk";

export type BulkChange = { price: number } | { availability: Availability };

export function merchantBulkPrice(input: string, currencies: readonly string[]): { price: number } | string {
  const unique = [...new Set(currencies)];
  if (unique.length !== 1) return "Select listings in one currency to change their price.";
  const currency = unique[0];
  const price = currency === "SAT" ? (/^\d+$/.test(input.trim()) ? Number(input.trim()) : null) : toMinor(input, currency);
  if (price == null || !Number.isSafeInteger(price) || price <= 0) return "Enter a valid price above zero in the selected currency.";
  return { price };
}

export function bulkEditBody(ids: string[], change: BulkChange): { ids: string[] } & BulkChange {
  return { ids, ...change };
}
