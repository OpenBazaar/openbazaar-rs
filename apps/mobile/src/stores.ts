// Store pages: the store list, one store's profile and listings, and store
// images. Every field shown comes from a record the store key signed and
// this page verified; an image is returned only if its bytes hash to the
// hash in that record. Paired client only.
import { pairedJson, pairedResponse, type ListingHeader } from "./catalog.ts";
import { sha256Hex, verifiedListings, verifiedStore, type StoreSummary } from "./records.ts";

export type { StoreSummary } from "./records.ts";

/** Same cap as the node. */
export const MAX_MEDIA_BYTES = 2 * 1024 * 1024;

export type StoreProfile = StoreSummary & {
  about: string;
  banner: string | null;
  policies: { shipping: string; returns: string; processingTime: string };
  /** ISO 3166-1 alpha-2 codes. */
  shipsTo: string[];
  /** YYYYMMDD, or null for a store record that predates profiles. */
  createdOn: number | null;
  mailboxKey: string | null;
  onion: string | null;
  /** This node's own store only: edits wait for an unlock to be signed. Not signed. */
  unpublishedChanges: boolean;
};

export type StoreModerator = { id: string; name: string | null };

export type StorePage = {
  store: StoreProfile;
  listings: ListingHeader[];
  moderators: StoreModerator[];
  /** Listings dropped because their record did not verify or belongs to another store. */
  rejected: number;
};

export type StoreList = { stores: StoreSummary[]; rejected: number };

export type StoreImage = { contentType: string; bytes: Uint8Array };

const UNREADABLE = "The shop sent a store the page could not read.";

function recordOf(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object") throw new Error(UNREADABLE);
  return value as Record<string, unknown>;
}

export function parseStoreList(value: unknown): StoreList {
  const items = recordOf(value).stores;
  const list = Array.isArray(items) ? items : [];
  const stores = list.map(verifiedStore).filter((store): store is StoreSummary => store !== null);
  return { stores, rejected: list.length - stores.length };
}

/// A store page, or an error when the store's own record does not verify
/// or is not the store that was asked for.
export function parseStorePage(value: unknown, key: string): StorePage {
  const body = recordOf(value);
  const summary = verifiedStore(body.store);
  if (!summary || summary.key !== key) throw new Error("This store's record is not signed by its key.");
  const details = summary.record.details;
  const names = new Map<string, string>();
  for (const item of Array.isArray(body.moderators) ? body.moderators : []) {
    const moderator = item && typeof item === "object" ? (item as Record<string, unknown>) : {};
    if (typeof moderator.id === "string" && typeof moderator.name === "string") names.set(moderator.id, moderator.name);
  }
  const { listings, rejected } = verifiedListings(body.listings, new Set([key]), new Set(summary.acceptsXmr ? [key] : []));
  return {
    store: {
      ...summary,
      about: details?.about ?? "",
      banner: details?.banner ?? null,
      policies: {
        shipping: details?.shippingPolicy ?? "",
        returns: details?.returnsPolicy ?? "",
        processingTime: details?.processingTime ?? "",
      },
      shipsTo: details?.shipsTo ?? [],
      createdOn: details?.createdOn ?? null,
      mailboxKey: details?.mailboxKey ?? null,
      onion: details?.onion ?? null,
      unpublishedChanges: summary.own && recordOf(body.store).unpublishedChanges === true,
    },
    listings,
    moderators: (details?.moderators ?? []).map((id) => ({ id, name: names.get(id) ?? null })),
    rejected,
  };
}

/// Image bytes, if they are what `hash` names and small enough.
export function checkStoreImage(hash: string, contentType: string, bytes: Uint8Array): StoreImage | null {
  if (bytes.length > MAX_MEDIA_BYTES || !contentType.startsWith("image/") || sha256Hex(bytes) !== hash) return null;
  return { contentType, bytes };
}

export async function fetchStores(): Promise<StoreList> {
  return parseStoreList(await pairedJson("/stores"));
}

export async function fetchStore(key: string): Promise<StorePage> {
  if (!/^[0-9a-f]{64}$/.test(key)) throw new Error("That is not a store key.");
  return parseStorePage(await pairedJson(`/stores/${key}`), key);
}

/// An avatar or banner by the hash in a verified store record.
export async function fetchStoreImage(hash: string): Promise<StoreImage> {
  if (!/^[0-9a-f]{64}$/.test(hash)) throw new Error("That is not an image hash.");
  const response = await pairedResponse(`/media/${hash}`);
  if (!response.ok) throw new Error(response.status === 404 ? "This node does not have that image." : "The image could not be loaded.");
  const bytes = new Uint8Array(await response.arrayBuffer());
  const image = checkStoreImage(hash, response.headers.get("Content-Type") ?? "", bytes);
  if (!image) throw new Error("The image the node sent is not the one the store signed.");
  return image;
}
