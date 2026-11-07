// The phone's copy of its home node, kept current by POST /remote/sync
// (docs/home-node-protocol.md, "Sync"). Pure: no storage, no network.
//
// Merge rule: per collection, apply `remove`, then `upsert`, then `doc`, and
// keep the node's etags. A collection that answers with `error` is left as it
// was. The phone never computes an etag.

export type Etagged = { etag: string; value: unknown };
export type CollectionError = { status: number; code: string };
export type CollectionReplica = {
  doc: Etagged | null;
  items: Record<string, Etagged>;
  error: CollectionError | null;
  syncedAt: number | null;
};
export type Replica = {
  version: 1;
  collections: Record<string, CollectionReplica>;
  node: Record<string, unknown> | null;
  serverTime: number | null;
};

/** Where each collection keeps its items in the node's GET answer. */
export const ITEM_COLLECTIONS: Record<string, { at: string; id: string }> = {
  catalog: { at: "listings", id: "hash" },
  orders: { at: "orders", id: "id" },
  stores: { at: "stores", id: "id" },
};
export const COLLECTIONS = ["catalog", "orders", "stores", "wallet", "lock", "network"] as const;

export function emptyReplica(): Replica {
  return { version: 1, collections: {}, node: null, serverTime: null };
}

type Have = { doc?: string; items?: Record<string, string> };

/** The `have` map for the sync request. Collections never synced are left out so they come back whole. */
export function haveOf(replica: Replica): Record<string, Have> {
  const have: Record<string, Have> = {};
  for (const [name, collection] of Object.entries(replica.collections)) {
    if (collection.syncedAt === null) continue;
    const entry: Have = {};
    if (collection.doc) entry.doc = collection.doc.etag;
    if (name in ITEM_COLLECTIONS) {
      entry.items = Object.fromEntries(Object.entries(collection.items).map(([id, item]) => [id, item.etag]));
    }
    have[name] = entry;
  }
  return have;
}

export type SyncChanges = { added: Record<string, string[]>; changed: Record<string, string[]>; removed: Record<string, string[]>; failed: string[] };

const record = (value: unknown): Record<string, unknown> | null =>
  value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;

function etagged(value: unknown): Etagged | null {
  const entry = record(value);
  return entry && typeof entry.etag === "string" && "value" in entry ? { etag: entry.etag, value: entry.value } : null;
}

/** One upsert entry: `{ id, etag, value }`; the id falls back to the item's own id field. */
function upsertEntry(name: string, value: unknown): { id: string; item: Etagged } | null {
  const item = etagged(value);
  if (!item) return null;
  const entry = record(value)!;
  const idKey = ITEM_COLLECTIONS[name]?.id ?? "id";
  const id = typeof entry.id === "string" ? entry.id : record(item.value)?.[idKey];
  return typeof id === "string" && id ? { id, item } : null;
}

function applyCollection(name: string, before: CollectionReplica | undefined, answer: Record<string, unknown>, at: number, changes: SyncChanges): CollectionReplica {
  const base: CollectionReplica = before ?? { doc: null, items: {}, error: null, syncedAt: null };
  const error = record(answer.error);
  if (error) {
    changes.failed.push(name);
    return { ...base, error: { status: typeof error.status === "number" ? error.status : 0, code: typeof error.code === "string" ? error.code : "error" } };
  }
  const upserts = Array.isArray(answer.upsert) ? answer.upsert.map(entry => upsertEntry(name, entry)) : [];
  if (upserts.some(entry => entry === null)) {
    changes.failed.push(name);
    return { ...base, error: { status: 0, code: "unreadable" } };
  }
  const items = { ...base.items };
  const removed = (Array.isArray(answer.remove) ? answer.remove : []).filter((id): id is string => typeof id === "string" && id in items);
  for (const id of removed) delete items[id];
  const added: string[] = [];
  const changed: string[] = [];
  for (const { id, item } of upserts as { id: string; item: Etagged }[]) {
    (id in items ? changed : added).push(id);
    items[id] = item;
  }
  const doc = answer.doc === null || answer.doc === undefined ? base.doc : (etagged(answer.doc) ?? base.doc);
  if (added.length) changes.added[name] = added;
  if (changed.length) changes.changed[name] = changed;
  if (removed.length) changes.removed[name] = removed;
  return { doc, items, error: null, syncedAt: at };
}

/** The replica after a sync answer, and what changed. The input is not modified. */
export function applySync(replica: Replica, answer: unknown): { replica: Replica; changes: SyncChanges } {
  const changes: SyncChanges = { added: {}, changed: {}, removed: {}, failed: [] };
  const body = record(answer);
  const collections = record(body?.collections);
  if (!body || !collections) return { replica, changes: { ...changes, failed: ["*"] } };
  const serverTime = typeof body.serverTime === "number" ? body.serverTime : replica.serverTime;
  const at = serverTime ?? 0;
  const next: Record<string, CollectionReplica> = { ...replica.collections };
  for (const [name, value] of Object.entries(collections)) {
    const collection = record(value);
    if (collection) next[name] = applyCollection(name, replica.collections[name], collection, at, changes);
  }
  return { replica: { version: 1, collections: next, node: record(body.node) ?? replica.node, serverTime }, changes };
}

/** The collection as the node's GET would answer it: the doc with its item array put back. */
export function collectionValue(replica: Replica, name: string): Record<string, unknown> | null {
  const collection = replica.collections[name];
  if (!collection || collection.syncedAt === null) return null;
  const doc = record(collection.doc?.value) ?? {};
  const where = ITEM_COLLECTIONS[name];
  if (!where) return collection.doc ? doc : null;
  return { ...doc, [where.at]: Object.values(collection.items).map(item => item.value) };
}
