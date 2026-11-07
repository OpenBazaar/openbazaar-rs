// The seller side of the paired node: the storefront, its listings, its
// sales, and the stores this node follows. Own listings come back as the
// signed records the node serves and are checked here like any other.
import { paired, pairedResponse, type BuyerOrder, type ContentRating, type ListingHeader, type ListingKind, parseBuyerOrder } from "./catalog.ts";
import { verifiedListings } from "./records.ts";
import type { Commerce } from "./commerce.ts";
import { forKind } from "./commerceDraft.ts";

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
  /** This node can take Monero for its own store's sales. */
  xmrAvailable: boolean;
  /** This node can run escrow for its own store's sales. */
  escrowAvailable: boolean;
  published: boolean;
  /** The seller has saved the storefront at least once. */
  setUp: boolean;
  unpublishedChanges: boolean;
  listingCount: number;
  network: string;
};

export type ProfileChanges = Partial<
  Pick<SellerProfile, "name" | "region" | "tagline" | "about" | "avatar" | "banner" | "shippingPolicy" | "returnsPolicy" | "processingTime" | "shipsTo">
>;

/** "SAT" prices in sats; the rest in the currency's minor unit. */
export type PriceCurrency = "SAT" | "USD" | "EUR" | "GBP" | "JPY";
export const PRICE_CURRENCIES: readonly PriceCurrency[] = ["USD", "EUR", "GBP", "JPY", "SAT"];
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
  fulfillment: { carrier: string; tracking: string; note: string; content: string; at: number } | null;
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
    xmrAvailable: p.xmrAvailable === true,
    escrowAvailable: p.escrowAvailable === true,
    published: p.published === true,
    setUp: p.setUp === true,
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
    fulfillment: f ? { carrier: text(f.carrier), tracking: text(f.tracking), note: text(f.note), content: text(f.content), at: count(f.at) } : null,
  };
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

/// Upload one image; the node answers its SHA-256, which records name.
export async function uploadImage(file: Blob): Promise<string> {
  const response = await pairedResponse("/media", { method: "POST", headers: { "Content-Type": file.type || "application/octet-stream" }, body: file });
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

export async function fetchFollows(): Promise<Follow[]> {
  return parseFollows(await paired("/follows"));
}

export async function followStore(url: string): Promise<Follow[]> {
  return parseFollows(await paired("/follows", json("POST", { url })));
}

export async function unfollowStore(url: string): Promise<Follow[]> {
  return parseFollows(await paired("/follows", json("DELETE", { url })));
}
