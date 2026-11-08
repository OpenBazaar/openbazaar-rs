import type { Availability } from "./commerce.ts";

/** Where a bulk edit is signed. Drafts are not published from here. */
export const BULK_PATH = "/store/listings/bulk";

export type BulkChange = { price: number } | { availability: Availability };

export function bulkEditBody(ids: string[], change: BulkChange): { ids: string[] } & BulkChange {
  return { ids, ...change };
}
