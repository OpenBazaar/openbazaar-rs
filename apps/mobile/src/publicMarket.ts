// The public market: every directory-admitted store, read by this browser.
//
// The admission snapshot is store ids the directory key signed. Each store's
// onion is metadata beside that snapshot, not a listing. This page then asks
// that store's own `GET /public/store` and checks the records itself. It does
// not ask our server to forward catalogs, photos, or descriptions.
import type { ListingHeader, ShopCatalog } from "./catalog.ts";
import { DIRECTORY_KEY, verifyAdmissionIndex } from "./catalogIndex.ts";
import { appDownload } from "./appDownload.ts";
import { catalogFromPublic, DIRECT_GET, storeBase, type PageAddress } from "./directStore.ts";

/// The moderator sale onion (`TEST_NETWORK_MODERATOR_ONION`). Used when this
/// page cannot ask the node which onion serves the market.
export const MODERATOR_ONION = "yc546t4ecasnoollz3voqrrqsevup3wk25g4d7mxaryfomtu7xh37ead.onion";

/// Introduction onions from crates/node/src/seeds.rs. Bootstrap only.
const INTRODUCTIONS = [
  "a5kon2xfzc2myexkxvvrdcw73jare5ptov4cyu3zp6alfcthge7ilmqd.onion",
  "z2arrch7sx44bkuz54s5m2ardeopml2xdbuifun6ubhxgocac53jbxid.onion",
];

const ONION = /^[a-z2-7]{56}\.onion$/;
const EMPTY_ID = "00".repeat(32);

export const NODE_REQUIRED = "Buying and selling run on your own node. This page does not take a card and does not pay the seller.";

export type NodeGate = { title: string; text: string; href: string; label: string };

/** The buy or sell control: the explanation, and the app download for this browser. */
export function nodeRequired(kind: "buy" | "sell", userAgent: string): NodeGate {
  const download = appDownload(userAgent);
  return {
    title: kind === "buy" ? "Buying runs on your node" : "Selling runs on your node",
    text: NODE_REQUIRED,
    href: download.href,
    label: download.label,
  };
}

export type MarketNotice = { sentence: string; href: string };

/**
 * Why this page cannot load onion stores, in one sentence, with the onion to
 * open. Null when this page is already that onion.
 */
export function marketNotice(protocol: string, hostname: string, onionHost: string): MarketNotice | null {
  const host = onionHost.toLowerCase();
  if (!ONION.test(host)) return null;
  const href = `http://${host}/`;
  if (protocol === "https:") {
    return { sentence: `This page is https, so it cannot load onion stores; open the market in Tor Browser at ${href}.`, href };
  }
  if (protocol === "http:" && hostname.toLowerCase().endsWith(".onion")) return null;
  return { sentence: `Onion stores load in Tor Browser at ${href}.`, href };
}

function emptyCatalog(): ShopCatalog {
  return {
    store: { id: EMPTY_ID, name: "OpenBazaar", region: "", contentRating: "general" },
    stores: [],
    moderator: null,
    listings: [],
    rejected: 0,
    lightningBlocked: null,
    lightningNetwork: null,
    xmrNetwork: null,
  };
}

/** One market from public store bodies. A store that does not verify is skipped. */
export function aggregatePublicStores(bodies: readonly unknown[]): ShopCatalog {
  const stores: ShopCatalog["stores"] = [];
  const listings: ListingHeader[] = [];
  const seen = new Set<string>();
  let rejected = 0;
  for (const body of bodies) {
    let catalog: ShopCatalog;
    try {
      catalog = catalogFromPublic(body);
    } catch {
      rejected += 1;
      continue;
    }
    const store = catalog.stores[0];
    if (!store) {
      rejected += catalog.rejected;
      continue;
    }
    if (!stores.some((item) => item.key === store.key)) stores.push(store);
    for (const listing of catalog.listings) {
      if (seen.has(listing.hash)) continue;
      seen.add(listing.hash);
      listings.push(listing);
    }
    rejected += catalog.rejected;
  }
  const first = stores[0];
  return {
    store: first
      ? { id: first.key, name: first.name, region: first.region, contentRating: first.contentRating }
      : emptyCatalog().store,
    stores,
    moderator: null,
    listings,
    rejected,
    lightningBlocked: null,
    lightningNetwork: null,
    xmrNetwork: null,
  };
}

export type StoreAddress = { storeId: string; url: string };

/** Addresses whose store id the snapshot named, and whose URL is an onion or https node. */
export function admittedAddresses(ids: readonly string[], rows: readonly StoreAddress[]): StoreAddress[] {
  const named = new Set(ids);
  const out: StoreAddress[] = [];
  for (const row of rows) {
    const url = storeBase(row.url);
    if (!named.has(row.storeId) || !url || out.some((item) => item.storeId === row.storeId)) continue;
    out.push({ storeId: row.storeId, url });
  }
  return out;
}

type Getter = (url: string, init?: RequestInit) => Promise<Response>;

function publicGet(): RequestInit {
  const signal = typeof AbortSignal !== "undefined" && typeof AbortSignal.timeout === "function" ? AbortSignal.timeout(20_000) : undefined;
  return signal ? { ...DIRECT_GET, signal } : { ...DIRECT_GET };
}

async function readJson(response: Response, limit: number): Promise<unknown | null> {
  const claimed = Number(response.headers.get("content-length") ?? "0");
  if (Number.isFinite(claimed) && claimed > limit) return null;
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > limit) return null;
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
}

async function readAdmission(base: string, fetcher: Getter): Promise<string[] | null> {
  try {
    const response = await fetcher(`${base}/public/admission-index`, publicGet());
    if (!response.ok) return null;
    const body = await readJson(response, 1024 * 1024);
    const record = body && typeof body === "object" ? (body as { record?: unknown }).record : null;
    return verifyAdmissionIndex(record, DIRECTORY_KEY);
  } catch {
    return null;
  }
}

async function readAddresses(base: string, fetcher: Getter): Promise<StoreAddress[]> {
  try {
    const response = await fetcher(`${base}/public/admission-addresses`, publicGet());
    if (!response.ok) return [];
    const body = await readJson(response, 1024 * 1024);
    const rows = body && typeof body === "object" ? (body as { stores?: unknown }).stores : null;
    if (!Array.isArray(rows)) return [];
    return rows.flatMap((row) => {
      if (!row || typeof row !== "object") return [];
      const item = row as { storeId?: unknown; url?: unknown };
      if (typeof item.storeId !== "string" || typeof item.url !== "string") return [];
      return [{ storeId: item.storeId, url: item.url }];
    });
  } catch {
    return [];
  }
}

async function saleOnion(loc: PageAddress, fetcher: Getter): Promise<string> {
  const host = loc.hostname.toLowerCase();
  if (loc.protocol === "http:" && ONION.test(host)) return host;
  try {
    const response = await fetcher("/remote/hello", publicGet());
    const body = await readJson(response, 8192);
    const onion = body && typeof body === "object" ? (body as { saleOnion?: unknown }).saleOnion : null;
    if (typeof onion === "string" && ONION.test(onion)) return onion;
  } catch {
    // The compiled moderator onion is the public market when this page has no node.
  }
  return MODERATOR_ONION;
}

async function mapPool<T>(items: readonly T[], limit: number, run: (item: T) => Promise<void>): Promise<void> {
  let next = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const item = items[next];
      next += 1;
      await run(item);
    }
  });
  await Promise.all(workers);
}

export type LoadedMarket = {
  catalog: ShopCatalog;
  notice: string | null;
  images: { url: string; hashes: string[] }[];
};

/**
 * Try `GET /public/catalog` on a moderator/intro node. Returns the signed
 * store bodies when the endpoint exists, or `null` so the caller falls back
 * to per-store fetching. Every record is verified client-side — zero extra
 * trust in the moderator.
 */
async function readPublicCatalog(base: string, fetcher: Getter): Promise<unknown[] | null> {
  try {
    const response = await fetcher(`${base}/public/catalog`, publicGet());
    if (!response.ok) return null;
    const body = await readJson(response, 8 * 1024 * 1024);
    if (!body || typeof body !== "object") return null;
    const stores = (body as { stores?: unknown }).stores;
    if (!Array.isArray(stores)) return null;
    return stores;
  } catch {
    return null;
  }
}

/** The market for this page. An https page does not fetch onion catalogs. */
export async function loadPublicMarket(loc: PageAddress, fetcher: Getter = globalThis.fetch): Promise<LoadedMarket> {
  const onion = await saleOnion(loc, fetcher);
  const notice = marketNotice(loc.protocol, loc.hostname, onion);
  if (notice) return { catalog: emptyCatalog(), notice: notice.sentence, images: [] };
  const bases = [`http://${loc.hostname.toLowerCase()}`, ...INTRODUCTIONS.map((host) => `http://${host}`)];

  // Fast path: one request to the moderator for every admitted store's
  // signed records. Falls back gracefully when the endpoint is unavailable
  // (e.g. an older node that does not serve /public/catalog).
  for (const base of bases) {
    const entries = await readPublicCatalog(base, fetcher);
    if (entries && entries.length > 0) {
      const bodies: unknown[] = [];
      const images: LoadedMarket["images"] = [];
      for (const entry of entries) {
        try {
          const catalog = catalogFromPublic(entry);
          const store = catalog.stores[0];
          if (!store) continue;
          bodies.push(entry);
          images.push({ url: base, hashes: catalog.listings.flatMap((listing) => listing.images) });
        } catch {
          // A single bad entry does not break the whole catalog.
        }
      }
      if (bodies.length > 0) {
        return { catalog: aggregatePublicStores(bodies), notice: null, images };
      }
    }
  }

  // Slow path: admission index → per-store fetch (N onion requests).
  let directory: { base: string; ids: string[] } | null = null;
  for (const base of bases) {
    const ids = await readAdmission(base, fetcher);
    if (ids) {
      directory = { base, ids };
      break;
    }
  }
  if (!directory) return { catalog: emptyCatalog(), notice: null, images: [] };
  const chosen = admittedAddresses(directory.ids, await readAddresses(directory.base, fetcher));
  const bodies: unknown[] = [];
  const images: LoadedMarket["images"] = [];
  await mapPool(chosen, 4, async (row) => {
    try {
      const response = await fetcher(`${row.url}/public/store`, publicGet());
      if (!response.ok) return;
      const body = await readJson(response, 4 * 1024 * 1024);
      if (!body) return;
      const catalog = catalogFromPublic(body);
      if (catalog.stores[0]?.key !== row.storeId) return;
      bodies.push(body);
      images.push({ url: row.url, hashes: catalog.listings.flatMap((listing) => listing.images) });
    } catch {
      // One store that does not answer stays out of the grid.
    }
  });
  return { catalog: aggregatePublicStores(bodies), notice: null, images };
}
