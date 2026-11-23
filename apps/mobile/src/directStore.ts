// A buyer with a store link and no node of their own.
//
// The seller's node is the server: `GET {url}/public/store` and
// `GET {url}/public/media/{hash}`. Records are kept only when the signature
// check this page already uses succeeds, and when the text is plain the way
// a node copying the store requires. What is shown then goes through
// `displayText`. Nothing here pays, holds coins, or publishes a listing.

import type { Commerce } from "./commerce.ts";
import type { ListingHeader, ShopCatalog } from "./catalog.ts";
import { verifyCatalogIndex } from "./catalogIndex.ts";
import { verifyListingRecord, verifyStoreRecord, type ListingRecord, type StoreRecord, type StoreSummary } from "./records.ts";
import { displayText } from "./safeText.ts";

const ONION = /^[a-z2-7]{56}\.onion$/;
const MAX_BODY = 4 * 1024 * 1024;
const MAX_LISTINGS = 500;

export const STORE_KEY = "ob-browse-store";

export const NOT_A_STORE = "That is not a store address. Paste http:// and a .onion name, or https:// and the seller's node.";
export const UNREACHABLE = "This page could not reach that store. An onion address has to be opened in Tor Browser. An https address has to be the seller's own node.";
export const UNSIGNED = "This store's record is not signed by its key.";
export const HOSTILE = "This store's text is not shown.";
export const PAYMENT_NOTE = "Your bag stays on this device. Paying needs a wallet on your own node. This page does not hold coins and does not pay the seller.";

/** A browser read of a public route: no cookies, no pairing header, no redirect. */
export const DIRECT_GET: RequestInit = {
  method: "GET",
  cache: "no-store",
  credentials: "omit",
  redirect: "error",
  referrerPolicy: "no-referrer",
};

export type PageAddress = { search: string; hostname: string; protocol: string; port?: string };
export type StoreChoice = { url: string | null; rejected: boolean };

let browsing: string | null = null;
const imageHosts = new Map<string, string>();

/** Forget which store served each photo. */
export function clearListingImages(): void {
  imageHosts.clear();
}

/** Photos in a market come from the store that signed the listing, by hash. */
export function setListingImages(base: string, hashes: readonly string[]): void {
  const url = storeBase(base);
  if (!url) return;
  for (const hash of hashes) {
    if (/^[0-9a-f]{64}$/.test(hash)) imageHosts.set(hash, url);
  }
}

/** The store that served this photo, when the market remembered it. */
export function imageHost(hash: string): string | null {
  return imageHosts.get(hash) ?? null;
}

/** Remember which seller this page is reading, so images come from that node. */
export function setDirectBrowse(url: string | null): void {
  browsing = url ? storeBase(url) : null;
}

export function directBrowseBase(): string | null {
  return browsing;
}

/** Control characters and bidi overrides. A description may keep tabs and line breaks. */
export function isPlainText(text: string, multiline: boolean): boolean {
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (ch === "\t" || ch === "\n" || ch === "\r") {
      if (!multiline) return false;
      continue;
    }
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return false;
    if (code === 0x61c || code === 0x200e || code === 0x200f || (code >= 0x2028 && code <= 0x202e) || (code >= 0x2066 && code <= 0x2069)) return false;
  }
  return true;
}

function httpsHost(host: string): boolean {
  if (!/^[a-z0-9.-]+$/.test(host)) return false;
  const labels = host.split(".");
  if (labels.length < 2 || labels.some((label) => !label || label.startsWith("-") || label.endsWith("-"))) return false;
  return labels.some((label) => /[a-z]/.test(label));
}

/** The seller node's base URL, or null when `pasted` is not an onion or https store. */
export function storeBase(pasted: string): string | null {
  const text = pasted.trim();
  if (!text || /\s/.test(text)) return null;
  let url: URL;
  try {
    url = new URL(text.includes("://") ? text : `http://${text}`);
  } catch {
    return null;
  }
  if (url.username || url.password || url.search || url.hash) return null;
  const path = url.pathname.replace(/\/+$/, "") || "/";
  if (path !== "/" && path !== "/public/store") return null;
  const host = url.hostname.toLowerCase();
  if (host.endsWith(".onion")) {
    return url.protocol === "http:" && ONION.test(host) && (url.port === "" || url.port === "80") ? `http://${host}` : null;
  }
  if (url.protocol === "https:" && httpsHost(host) && url.port !== "0") {
    return `https://${host}${url.port ? `:${url.port}` : ""}`;
  }
  return null;
}

/** A store named in `?store=`. The page itself is the market, on an onion and on the web. */
export function storeFromLocation(loc: PageAddress, _homeNode: boolean): StoreChoice {
  const params = new URLSearchParams(loc.search);
  if (params.has("store")) {
    const url = storeBase(params.get("store") ?? "");
    return { url, rejected: url === null };
  }
  return { url: null, rejected: false };
}

/** The link's store, otherwise the one this browser opened last time. */
export function storeChoice(loc: PageAddress, homeNode: boolean, saved: string | null): StoreChoice {
  const opened = storeFromLocation(loc, homeNode);
  if (opened.rejected || opened.url) return opened;
  return { url: saved ? storeBase(saved) : null, rejected: false };
}

export function directImageUrl(base: string, hash: string): string | null {
  const url = storeBase(base);
  if (!url || !/^[0-9a-f]{64}$/.test(hash)) return null;
  return `${url}/public/media/${hash}`;
}

function everyString(value: unknown, visit: (text: string) => boolean): boolean {
  if (typeof value === "string") return visit(value);
  if (Array.isArray(value)) return value.every((item) => everyString(item, visit));
  if (value && typeof value === "object") return Object.values(value).every((item) => everyString(item, visit));
  return true;
}

function storeTextOk(record: StoreRecord): boolean {
  if (![record.name, record.region, record.bolt12Offer, record.silentPaymentCode].every((text) => isPlainText(text, false))) return false;
  const details = record.details;
  if (!details) return true;
  const single = [details.tagline, details.processingTime, details.mailboxKey, details.onion ?? ""];
  if (!single.every((text) => isPlainText(text, false))) return false;
  if (![details.about, details.shippingPolicy, details.returnsPolicy].every((text) => isPlainText(text, true))) return false;
  return details.shipsTo.every((code) => isPlainText(code, false)) && details.moderators.every((id) => isPlainText(id, false));
}

function listingTextOk(record: ListingRecord): boolean {
  if (![record.title, record.category, record.shipFrom, record.seller, record.product].every((text) => isPlainText(text, false))) return false;
  if (!isPlainText(record.description, true)) return false;
  if (record.price && !isPlainText(record.price.currency, false)) return false;
  if (record.contentTags.some((tag) => !isPlainText(tag, false))) return false;
  return record.commerce ? everyString(record.commerce, (text) => isPlainText(text, false)) : true;
}

function hashOrNull(value: string | null): string | null {
  return value && /^[0-9a-f]{64}$/.test(value) ? value : null;
}

/** The store summary to show, or null when its text is not plain. */
export function shownStore(record: StoreRecord, listingCount: number): StoreSummary | null {
  if (!storeTextOk(record)) return null;
  const details = record.details;
  const safeDetails = details
    ? {
        ...details,
        tagline: displayText(details.tagline, 120),
        about: displayText(details.about, 2_000),
        shippingPolicy: displayText(details.shippingPolicy, 1_000),
        returnsPolicy: displayText(details.returnsPolicy, 1_000),
        processingTime: displayText(details.processingTime, 120),
        onion: details.onion ? displayText(details.onion, 80) || null : null,
        avatar: hashOrNull(details.avatar),
        banner: hashOrNull(details.banner),
        shipsTo: details.shipsTo.map((code) => displayText(code, 8)).filter(Boolean),
      }
    : null;
  const name = displayText(record.name, 64) || "Store";
  const region = displayText(record.region, 64);
  return {
    key: record.key,
    name,
    region,
    tagline: safeDetails?.tagline ?? "",
    avatar: safeDetails?.avatar ?? null,
    contentRating: record.contentRating,
    acceptsXmr: safeDetails?.acceptsXmr ?? false,
    acceptsLightning: safeDetails?.acceptsLightning ?? false,
    acceptsZec: safeDetails?.acceptsZec ?? false,
    listingCount,
    own: false,
    version: record.version,
    record: { ...record, name, region, details: safeDetails },
  };
}

function shownCommerce(commerce: Commerce): Commerce {
  const clean = (text: string, max: number) => displayText(text, max);
  return {
    ...commerce,
    conditionNote: clean(commerce.conditionNote, 2_000),
    specifics: Object.fromEntries(Object.entries(commerce.specifics).map(([key, value]) => [clean(key, 80), clean(value, 200)])),
    axes: commerce.axes.map((axis) => ({ name: clean(axis.name, 80), values: axis.values.map((value) => clean(value, 80)) })),
    variants: commerce.variants.map((variant) => ({ ...variant, options: variant.options.map((value) => clean(value, 80)), sku: clean(variant.sku, 80) })),
    shipping: {
      ...commerce.shipping,
      pickup: commerce.shipping.pickup ? clean(commerce.shipping.pickup, 160) || null : null,
      services: commerce.shipping.services.map((service) => ({ ...service, name: clean(service.name, 80), regions: service.regions.map((code) => clean(code, 8)) })),
    },
  };
}

/** The listing card to show, or null when the text is not plain or the listing is not this store's. */
export function shownListing(record: ListingRecord, store: StoreRecord): ListingHeader | null {
  if (record.storeId !== store.key || record.seller !== store.name || !listingTextOk(record)) return null;
  const description = displayText(record.description, 20_000);
  return {
    hash: record.hash,
    storeId: record.storeId,
    title: displayText(record.title, 512),
    category: displayText(record.category, 160),
    priceSats: record.priceSats,
    shipFrom: displayText(record.shipFrom, 160),
    kind: record.kind,
    acceptsModerator: record.moderated,
    acceptsXmr: store.details?.acceptsXmr === true,
    acceptsLightning: store.details?.acceptsLightning === true,
    acceptsZec: store.details?.acceptsZec === true,
    description: description || undefined,
    seller: displayText(record.seller, 256),
    stock: record.stock,
    listedOn: record.listedOn,
    product: displayText(record.product, 128),
    shippingSats: record.shippingSats,
    freeOverSats: record.freeOverSats,
    contentRating: record.contentRating,
    contentTags: record.contentTags,
    listingId: record.listingId,
    images: record.images.filter((hash) => /^[0-9a-f]{64}$/.test(hash)),
    price: record.price,
    commerce: record.commerce ? shownCommerce(record.commerce) : null,
  };
}

/** Listing hashes the store's signed catalog index names, when that index matches this record. */
function namedByIndex(body: { catalogIndex?: unknown; store?: unknown }, storeKey: string): string[] | null {
  if (typeof body.catalogIndex !== "string" || typeof body.store !== "string") return null;
  return verifyCatalogIndex(body.catalogIndex, storeKey, body.store);
}

/** The shop catalog from one `GET /public/store` body. Unsigned or hostile listings are counted and dropped. A listing the signed catalog index leaves out is dropped too. */
export function catalogFromPublic(value: unknown): ShopCatalog {
  const body = value && typeof value === "object" ? (value as { store?: unknown; listings?: unknown; catalogIndex?: unknown }) : null;
  const listings = body && Array.isArray(body.listings) ? body.listings : null;
  const verified = body ? verifyStoreRecord(body.store) : null;
  if (!body || !listings || !verified) throw new Error(body && listings ? UNSIGNED : "That store's answer could not be read.");
  if (!storeTextOk(verified)) throw new Error(HOSTILE);
  if (listings.length > MAX_LISTINGS) throw new Error("That store sent too many listings.");
  const shown: ListingHeader[] = [];
  let rejected = 0;
  for (const item of listings) {
    const record = typeof item === "string" ? verifyListingRecord(item) : null;
    const card = record ? shownListing(record, verified) : null;
    if (card) shown.push(card);
    else rejected += 1;
  }
  const summary = shownStore(verified, shown.length);
  if (!summary) throw new Error(HOSTILE);
  const index = namedByIndex(body, verified.key);
  const named = index ? shown.filter((card) => index.includes(card.hash)) : shown;
  return {
    store: { id: summary.key, name: summary.name, region: summary.region, contentRating: summary.contentRating },
    stores: [{ ...summary, listingCount: named.length }],
    moderator: null,
    listings: named,
    rejected: rejected + shown.length - named.length,
    lightningBlocked: null,
    lightningNetwork: null,
    xmrNetwork: null,
  };
}

type Getter = (url: string, init?: RequestInit) => Promise<Response>;

function directInit(): RequestInit {
  const signal = typeof AbortSignal !== "undefined" && typeof AbortSignal.timeout === "function" ? AbortSignal.timeout(20_000) : undefined;
  return signal ? { ...DIRECT_GET, signal } : { ...DIRECT_GET };
}

/** Read the seller's public store. `fetcher` is `fetch` in the browser. */
export async function fetchPublicCatalog(base: string, fetcher: Getter = globalThis.fetch): Promise<ShopCatalog> {
  const url = storeBase(base);
  if (!url) throw new Error(NOT_A_STORE);
  let response: Response;
  try {
    response = await fetcher(`${url}/public/store`, directInit());
  } catch {
    throw new Error(UNREACHABLE);
  }
  const claimed = Number(response.headers.get("content-length") ?? "0");
  if (Number.isFinite(claimed) && claimed > MAX_BODY) throw new Error("That store's answer is too large.");
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > MAX_BODY) throw new Error("That store's answer is too large.");
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    throw new Error("That store's answer could not be read.");
  }
  if (!response.ok) {
    const message = parsed && typeof parsed === "object" && typeof (parsed as { error?: unknown }).error === "string" ? (parsed as { error: string }).error : "";
    if (response.status === 404 && message.toLowerCase().includes("not selling")) throw new Error("That node answered, but it has no open store yet.");
    throw new Error(UNREACHABLE);
  }
  return catalogFromPublic(parsed);
}
