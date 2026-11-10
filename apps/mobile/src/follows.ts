// The stores this node's directory holds, from `GET /directory`, and the two
// calls that change which of them it follows. Both answer with the new list.
import { paired } from "./catalog.ts";

/** `added` is true when this person follows the store. Without it the store is here because the directory names it. */
export type DirectoryStore = { storeId: string; name: string | null; listings: number; added: boolean };

export function storesOf(value: unknown): DirectoryStore[] {
  if (!value || typeof value !== "object" || !("stores" in value)) return [];
  const rows = (value as { stores?: unknown }).stores;
  if (!Array.isArray(rows)) return [];
  return rows.flatMap(row => {
    if (!row || typeof row !== "object" || !("storeId" in row)) return [];
    const item = row as { storeId?: unknown; name?: unknown; listings?: unknown; added?: unknown };
    if (typeof item.storeId !== "string") return [];
    return [{
      storeId: item.storeId,
      name: typeof item.name === "string" ? item.name : null,
      listings: typeof item.listings === "number" ? item.listings : 0,
      added: item.added === true,
    }];
  });
}

export function fetchDirectory(): Promise<DirectoryStore[]> {
  return paired("/directory").then(storesOf);
}

function change(method: "POST" | "DELETE", storeId: string): Promise<DirectoryStore[]> {
  return paired("/follow", { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify({ storeId }) }).then(storesOf);
}

export function followStore(storeId: string): Promise<DirectoryStore[]> {
  return change("POST", storeId);
}

export function unfollowStore(storeId: string): Promise<DirectoryStore[]> {
  return change("DELETE", storeId);
}

/**
 * What a change did, read from the list the node sent back. A followed store
 * stays when the directory stops naming it. An unfollowed store stays only
 * while the directory names it.
 */
export function followNotice(stores: readonly DirectoryStore[], storeId: string, name: string): string {
  const store = stores.find(item => item.storeId === storeId);
  if (store?.added) return `Following ${name}. Your node keeps it even if the directory drops it.`;
  return store ? `Stopped following ${name}. The directory still names it.` : `Stopped following ${name}. Your node no longer shows it.`;
}
