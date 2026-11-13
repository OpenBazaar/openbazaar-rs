// The seller side of the paired node: the storefront, its listings, its
// sales, and the stores this node follows. Own listings come back as the
// signed records the node serves and are checked here like any other.
import { NodeApiError, paired, pairedResponse, type BuyerOrder, type ContentRating, type ListingHeader, type ListingKind, parseBuyerOrder } from "./catalog.ts";
import { verifiedListings } from "./records.ts";
import { formatZec } from "./zec.ts";
import type { Commerce } from "./commerce.ts";
import { forKind } from "./commerceDraft.ts";
import type { CouponBody } from "./couponDraft.ts";
import { BULK_PATH, bulkEditBody, type BulkChange } from "./bulkEdit.ts";
import { draftPath } from "./listingDraft.ts";
import { canShrink, shrinkImage } from "./imageShrink.ts";
import { parseShipmentTracking, type ShipmentTracking } from "./shipment.ts";

export type Role = "buyer" | "seller";

export type SellerProfile = {
  role: Role | null;
  storeKey: string;
  name: string;
  region: string;
  tagline: string;
  about: string;
  avatar: string | null;
  banner: string | null;
  shippingPolicy: string;
  returnsPolicy: string;
  processingTime: string;
  shipsTo: string[];
  contentRating: ContentRating;
  acceptsXmr: boolean;
  /** The signed store record takes Lightning direct pay. */
  acceptsLightning: boolean;
  /** This node can take Monero for its own store's sales. */
  xmrAvailable: boolean;
  /** This node can run escrow for its own store's sales. */
  escrowAvailable: boolean;
  published: boolean;
  /** The seller has saved the storefront at least once. */
  setUp: boolean;
  /** The store is paused. Not a place, a date, or a note. */
  away: boolean;
  unpublishedChanges: boolean;
  listingCount: number;
  network: string;
};

export type ProfileChanges = Partial<
  Pick<SellerProfile, "name" | "region" | "tagline" | "about" | "avatar" | "banner" | "shippingPolicy" | "returnsPolicy" | "processingTime" | "shipsTo">
>;

export type StoreImageField = "avatar" | "banner";

/** A PUT /store/profile body that sets or clears one storefront image. */
export function storeImageChange(field: StoreImageField, hash: string | null): ProfileChanges {
  if (hash !== null && !/^[0-9a-f]{64}$/.test(hash)) throw new Error("The node did not keep the image.");
  return { [field]: hash };
}

/** "SAT" prices in sats; the rest in the currency's minor unit. */
export type PriceCurrency = "SAT" | "USD" | "EUR" | "GBP" | "JPY";
export const PRICE_CURRENCIES: readonly PriceCurrency[] = ["USD", "EUR", "GBP", "JPY", "SAT"];
/** Bytes in the signed store record's display name. */
export const MAX_STORE_NAME = 64;

/** Why `name` cannot be signed, or null when it can. Empty names are refused. */
export function displayNameError(name: string): string | null {
  const text = name.trim();
  if (!text) return "Give the store a name.";
  if (new TextEncoder().encode(text).length > MAX_STORE_NAME) return `The name is at most ${MAX_STORE_NAME} characters.`;
  return null;
}

export const CONTENT_TAGS = ["nudity", "violence", "drugs", "weapons", "gambling"] as const;
export const MAX_LISTING_IMAGES = 8;

export type ListingDraft = {
  title: string;
  description: string;
  category: string;
  kind: ListingKind;
  shipFrom: string;
  stock: number;
  images: string[];
  currency: PriceCurrency;
  price: number;
  shipping: number;
  contentRating: Exclude<ContentRating, "unrated">;
  contentTags: string[];
  /** Options, condition and shipping services; signing with these makes a version 7 listing. */
  commerce?: Commerce | null;
};

export type SaleLine = { listingHash: string; listingId: string | null; title: string; kind: ListingKind; quantity: number; priceSats: number };

export type Sale = {
  id: string;
  created: number;
  status: string;
  lines: SaleLine[];
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  address: string;
  shipTo: string;
  deliveryNote: string;
  receivedSats: number;
  confirmations: number;
  txids: string[];
  /** The transaction the buyer's node says it sent, before the chain shows it. */
  sentTxid: string | null;
  network: string;
  /** A sale paid in Zcash: `address` is empty and the amounts are in ZEC. `expired`: the quote lapsed unpaid, so the sale stays unpaid whatever arrives. */
  zcash: { address: string; amountZec: string; receivedZec: string; expired: boolean } | null;
  fulfillment: { carrier: string; tracking: string; note: string; content: string; at: number } | null;
  tracking: ShipmentTracking | null;
};

export type Fulfill = { carrier?: string; tracking?: string; note?: string; content?: string };

export type Follow = { url: string; storeId: string | null; name: string | null; listings: number; fetched: number | null; error: string | null };

const UNREADABLE = "The node sent a store record the page could not read.";

function record(value: unknown, message = UNREADABLE): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(message);
  return value as Record<string, unknown>;
}
const text = (value: unknown) => (typeof value === "string" ? value : "");
const count = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0);
const hash = (value: unknown) => (typeof value === "string" && /^[0-9a-f]{64}$/.test(value) ? value : null);
const RATINGS: readonly ContentRating[] = ["general", "sensitive", "adult", "unrated"];

export function parseProfile(value: unknown): SellerProfile {
  const p = record(value);
  if (typeof p.storeKey !== "string" || !/^[0-9a-f]{64}$/.test(p.storeKey)) throw new Error(UNREADABLE);
  return {
    role: p.role === "buyer" || p.role === "seller" ? p.role : null,
    storeKey: p.storeKey,
    name: text(p.name),
    region: text(p.region),
    tagline: text(p.tagline),
    about: text(p.about),
    avatar: hash(p.avatar),
    banner: hash(p.banner),
    shippingPolicy: text(p.shippingPolicy),
    returnsPolicy: text(p.returnsPolicy),
    processingTime: text(p.processingTime),
    shipsTo: Array.isArray(p.shipsTo) ? p.shipsTo.filter((c): c is string => typeof c === "string") : [],
    contentRating: RATINGS.includes(p.contentRating as ContentRating) ? (p.contentRating as ContentRating) : "unrated",
    acceptsXmr: p.acceptsXmr === true,
    acceptsLightning: p.acceptsLightning === true,
    xmrAvailable: p.xmrAvailable === true,
    escrowAvailable: p.escrowAvailable === true,
    published: p.published === true,
    setUp: p.setUp === true,
    away: p.away === true,
    unpublishedChanges: p.unpublishedChanges === true,
    listingCount: count(p.listingCount),
    network: text(p.network),
  };
}

/// Own listings, kept only if each record verifies against the own store key.
export function parseOwnListings(value: unknown, storeKey: string): ListingHeader[] {
  return verifiedListings(record(value).listings, new Set([storeKey])).listings;
}

export function parseSale(value: unknown): Sale {
  const s = record(value, "The node sent a sale the page could not read.");
  if (typeof s.id !== "string" || !/^[0-9a-f]+$/.test(s.id)) throw new Error("The node sent a sale the page could not read.");
  const f = s.fulfillment && typeof s.fulfillment === "object" ? (s.fulfillment as Record<string, unknown>) : null;
  const pay = s.payment && typeof s.payment === "object" ? (s.payment as Record<string, unknown>) : {};
  const zec = (value: unknown) => formatZec(BigInt(typeof value === "string" && /^\d{1,20}$/.test(value) ? value : "0"));
  return {
    id: s.id,
    created: count(s.created),
    status: text(s.status) || "Awaiting payment",
    lines: (Array.isArray(s.lines) ? s.lines : []).map((item) => {
      const line = record(item, "The node sent a sale the page could not read.");
      return {
        listingHash: text(line.listingHash),
        listingId: typeof line.listingId === "string" ? line.listingId : null,
        title: text(line.title) || "Item",
        kind: line.kind === "digital" ? "digital" : "physical",
        quantity: count(line.quantity),
        priceSats: count(line.priceSats),
      };
    }),
    itemsSats: count(s.itemsSats),
    shippingSats: count(s.shippingSats),
    totalSats: count(s.totalSats),
    address: text(s.address),
    shipTo: text(s.shipTo),
    deliveryNote: text(s.deliveryNote),
    receivedSats: count(s.receivedSats),
    confirmations: count(s.confirmations),
    txids: Array.isArray(s.txids) ? s.txids.filter((t): t is string => typeof t === "string" && /^[0-9a-f]{64}$/i.test(t)) : [],
    sentTxid: typeof s.sentTxid === "string" && /^[0-9a-f]{64}$/i.test(s.sentTxid) ? s.sentTxid : null,
    network: text(s.network),
    zcash: pay.rail === "zec" ? { address: text(pay.zcashAddress), amountZec: zec(pay.amountZatoshi), receivedZec: zec(pay.receivedZatoshi), expired: pay.expired === true } : null,
    fulfillment: f ? { carrier: text(f.carrier), tracking: text(f.tracking), note: text(f.note), content: text(f.content), at: count(f.at) } : null,
    tracking: parseShipmentTracking(s.tracking),
  };
}

const ONION_SEED = /^http:\/\/[a-z2-7]{56}\.onion$/;

/** Onion seeds from `GET /follows`. A clearnet sale URL is not a seed. */
export function testNetworkSeeds(value: unknown): string[] {
  const seeds = record(value).seeds;
  if (!Array.isArray(seeds)) return [];
  return seeds.flatMap((seed) => {
    const url = text(record(seed).url);
    return ONION_SEED.test(url) ? [url] : [];
  });
}

export function parseFollows(value: unknown): Follow[] {
  const items = record(value).follows;
  return (Array.isArray(items) ? items : []).map((item) => {
    const f = record(item);
    return {
      url: text(f.url),
      storeId: hash(f.storeId),
      name: typeof f.name === "string" ? f.name : null,
      listings: count(f.listings),
      fetched: typeof f.fetched === "number" ? f.fetched : null,
      error: typeof f.error === "string" ? f.error : null,
    };
  });
}

/** Sales the seller still has to act on: paid and not yet sent. */
export function salesToShip(sales: Sale[]): number {
  return sales.filter((sale) => sale.status === "Paid").length;
}

const MINOR_DIGITS: Record<Exclude<PriceCurrency, "SAT">, number> = { USD: 2, EUR: 2, GBP: 2, JPY: 0 };

/// A typed amount ("12.50") in the unit the node takes: minor units, or sats.
/// Null when it is not a plain non-negative number with at most the currency's decimals.
export function toMinorUnits(input: string, currency: PriceCurrency): number | null {
  const digits = currency === "SAT" ? 0 : MINOR_DIGITS[currency];
  const trimmed = input.trim().replace(/,/g, "");
  const pattern = digits ? new RegExp(`^\\d+(\\.\\d{0,${digits}})?$`) : /^\d+$/;
  if (!pattern.test(trimmed)) return null;
  const [whole, fraction = ""] = trimmed.split(".");
  const value = Number(whole) * 10 ** digits + Number(fraction.padEnd(digits, "0") || 0);
  return Number.isSafeInteger(value) ? value : null;
}

/// Minor units (or sats) back to the text a seller typed.
export function fromMinorUnits(amount: number, currency: PriceCurrency): string {
  const digits = currency === "SAT" ? 0 : MINOR_DIGITS[currency];
  return digits ? (amount / 10 ** digits).toFixed(digits) : String(amount);
}

export function formatPrice(amount: number, currency: string): string {
  if (currency === "SAT") return `${amount.toLocaleString("en-US")} sats`;
  const digits = MINOR_DIGITS[currency as keyof typeof MINOR_DIGITS] ?? 2;
  return new Intl.NumberFormat("en-US", { style: "currency", currency }).format(amount / 10 ** digits);
}

/// The editable fields of an own listing.
export function draftOf(listing: ListingHeader): ListingDraft {
  const fiat = listing.price;
  return {
    title: listing.title,
    description: listing.description ?? "",
    category: listing.category,
    kind: listing.kind,
    shipFrom: listing.shipFrom,
    stock: listing.stock,
    images: listing.images,
    currency: fiat ? (fiat.currency as PriceCurrency) : "SAT",
    price: fiat ? fiat.amount : listing.priceSats,
    shipping: fiat ? fiat.shipping : listing.shippingSats,
    contentRating: listing.contentRating === "unrated" ? "general" : listing.contentRating,
    contentTags: listing.contentTags,
    commerce: listing.commerce ?? null,
  };
}

function json(method: string, body: unknown): RequestInit {
  return { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
}

export async function fetchProfile(): Promise<SellerProfile> {
  return parseProfile(await paired("/store/profile"));
}

export async function updateProfile(changes: ProfileChanges): Promise<SellerProfile> {
  return parseProfile(await paired("/store/profile", json("PUT", changes)));
}

export async function setRole(role: Role): Promise<SellerProfile> {
  return parseProfile(await paired("/store/role", json("PUT", { role })));
}

/// Pause the store, or open it again. The body is only the flag: a note could say where the seller is.
export async function setVacation(away: boolean): Promise<SellerProfile> {
  return parseProfile(await paired("/store/vacation", json("PUT", { away })));
}

/** Bytes in one uploaded image: the node's cap, `MAX_MEDIA` in crates/node/src/media.rs. */
const MAX_IMAGE_BYTES = 2 * 1024 * 1024;
const IMAGE_TOO_LARGE = `That image is larger than ${MAX_IMAGE_BYTES / (1024 * 1024)} MB. Choose a smaller one.`;
const IMAGE_NOT_SHRUNK = `That image is larger than ${MAX_IMAGE_BYTES / (1024 * 1024)} MB and could not be shrunk to fit. Choose a smaller one.`;

/// The image to send: `file` as it is when it fits the cap, a shrunk copy
/// when it does not. The native app cannot shrink and refuses it.
async function imageThatFits(file: Blob): Promise<Blob> {
  if (file.size <= MAX_IMAGE_BYTES) return file;
  if (!canShrink()) throw new Error(IMAGE_TOO_LARGE);
  const shrunk = await shrinkImage(file, MAX_IMAGE_BYTES);
  if (!shrunk) throw new Error(IMAGE_NOT_SHRUNK);
  return shrunk;
}

/// Upload one image; the node answers its SHA-256, which records name.
/// An image over the cap is shrunk in the browser first.
export async function uploadImage(picked: Blob): Promise<string> {
  const file = await imageThatFits(picked);
  const response = await pairedResponse("/media", { method: "POST", headers: { "Content-Type": file.type || "application/octet-stream" }, body: file }).catch((error: unknown) => {
    // The node answers an oversized upload 413 with a plain-text body.
    throw error instanceof NodeApiError && error.status === 413 ? new Error(IMAGE_TOO_LARGE) : error;
  });
  const id = hash(record(await response.json()).hash);
  if (!id) throw new Error("The node did not keep the image.");
  return id;
}

export async function fetchOwnListings(storeKey: string): Promise<ListingHeader[]> {
  return parseOwnListings(await paired("/store/listings"), storeKey);
}

export async function saveListing(id: string | null, draft: ListingDraft): Promise<void> {
  const body = draft.kind === "digital" ? { ...draft, shipFrom: "", shipping: 0, commerce: draft.commerce ? forKind(draft.commerce, "digital") : null } : draft;
  await paired(id ? `/store/listings/${id}` : "/store/listings", json(id ? "PUT" : "POST", body));
}

export type SavedDraft = { draftId: string; listing: ListingDraft };

export { draftPath };

function parseDraft(value: unknown): SavedDraft {
  const row = record(value);
  const draftId = typeof row.draftId === "string" ? row.draftId : "";
  const listing = row.listing;
  if (!draftId || !listing || typeof listing !== "object") throw new Error("The draft could not be read.");
  return { draftId, listing: listing as ListingDraft };
}

export async function fetchDrafts(): Promise<SavedDraft[]> {
  const items = record(await paired("/store/drafts")).drafts;
  return (Array.isArray(items) ? items : []).map(parseDraft);
}

/// Keep the listing on the node without signing it or serving it to buyers.
export async function saveDraft(id: string | null, draft: ListingDraft): Promise<SavedDraft> {
  const body = draft.kind === "digital" ? { ...draft, shipFrom: "", shipping: 0, commerce: draft.commerce ? forKind(draft.commerce, "digital") : null } : draft;
  return parseDraft(await paired(draftPath(id), json(id ? "PUT" : "POST", body)));
}

/// Sign one price or one availability onto the selected listings. Drafts are not included.
export async function bulkEditListings(ids: string[], change: BulkChange): Promise<void> {
  await paired(BULK_PATH, json("POST", bulkEditBody(ids, change)));
}

export async function deleteDraft(id: string): Promise<void> {
  await paired(`/store/drafts/${id}`, { method: "DELETE" });
}

export type StoreCoupon = {
  code: string;
  percentBps: number | null;
  fixed: number | null;
  currency: string;
  minSubtotal: number;
  startsOn: number;
  endsOn: number;
  maxUses: number;
  uses: number;
};

export async function fetchCoupons(): Promise<StoreCoupon[]> {
  const items = record(await paired("/coupons")).coupons;
  return (Array.isArray(items) ? items : []).map(item => {
    const row = record(item);
    return {
      code: typeof row.code === "string" ? row.code : "",
      percentBps: typeof row.percentBps === "number" ? row.percentBps : null,
      fixed: typeof row.fixed === "number" ? row.fixed : null,
      currency: typeof row.currency === "string" ? row.currency : "",
      minSubtotal: typeof row.minSubtotal === "number" ? row.minSubtotal : 0,
      startsOn: typeof row.startsOn === "number" ? row.startsOn : 0,
      endsOn: typeof row.endsOn === "number" ? row.endsOn : 0,
      maxUses: typeof row.maxUses === "number" ? row.maxUses : 0,
      uses: typeof row.uses === "number" ? row.uses : 0,
    };
  });
}

/// Sign a coupon through the store's existing coupon path. It is not a listing field.
export async function saveCoupon(body: CouponBody): Promise<void> {
  await paired("/coupons", json("POST", body));
}

export async function deleteListing(id: string): Promise<void> {
  await paired(`/store/listings/${id}`, { method: "DELETE" });
}

export async function fetchSales(): Promise<Sale[]> {
  const items = record(await paired("/sales")).sales;
  return (Array.isArray(items) ? items : []).map(parseSale);
}

export async function fulfillSale(id: string, fulfill: Fulfill): Promise<Sale> {
  return parseSale(await paired(`/sales/${id}/fulfill`, json("POST", fulfill)));
}

/// The buyer confirms an order to another node's store arrived.
export async function completeOrder(id: string): Promise<BuyerOrder> {
  return parseBuyerOrder(await paired(`/orders/${id}/complete`, { method: "POST" }));
}
