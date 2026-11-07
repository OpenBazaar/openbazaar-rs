import { verifiedListings, verifiedStore, type StoreSummary } from "./records.ts";
import type { LabelerLabel } from "./safety/filter";
import { homeFetch, servedByHomeNode, servedByLocalShop, signedFetch, targetPrefix } from "./homeNode/transport.ts";
import { lnBlockedReason, lnStoreKeys, parseLnInvoice, type LnInvoice } from "./lightning.ts";

// The shop dials only this origin. A query parameter, settings field, seed,
// peer, or vendor address cannot point it anywhere else. Paired requests send
// the node's pairing token as X-Ob-Pair. The desktop app starts its own node
// on a free loopback port and sets __OB_NODE_ORIGIN__ and __OB_PAIR_TOKEN__
// before this script runs; only a 127.0.0.1 origin is taken. In a browser the
// node gives the token only for a one-time code printed in its terminal.
// A page served by a home node to a remote browser talks only to that origin
// and signs each request with its device key (src/homeNode/transport.ts).
// A page served by this machine's node (`ob-local-shop`) talks to that same
// origin and relies on the short-lived localhost cookie. It does not receive
// the pairing token.
const DESKTOP_NODE_ORIGIN = (globalThis as { __OB_NODE_ORIGIN__?: unknown }).__OB_NODE_ORIGIN__;
export const NODE_ORIGIN = servedByHomeNode()
  ? location.origin
  : typeof DESKTOP_NODE_ORIGIN === "string" && /^http:\/\/127\.0\.0\.1:\d{1,5}$/.test(DESKTOP_NODE_ORIGIN)
    ? DESKTOP_NODE_ORIGIN
    : servedByLocalShop()
      ? location.origin
      : "http://127.0.0.1:8787";

export const OFFLINE_MESSAGE =
  "The shop cannot be reached because your node is offline.";

const PAIR_HEADER = "X-Ob-Pair";
const TOKEN_PATTERN = /^[0-9a-fA-F]{64}$/;
const DESKTOP_TOKEN = (globalThis as { __OB_PAIR_TOKEN__?: unknown }).__OB_PAIR_TOKEN__;
const TOKEN_KEY = `ob-pair:${NODE_ORIGIN}`;

export class NodeOfflineError extends Error {
  constructor() {
    super(OFFLINE_MESSAGE);
    this.name = "NodeOfflineError";
  }
}

export type ListingKind = "physical" | "digital";

/** The vendor's signed self-label. "unrated" means the record predates labels. */
export type ContentRating = "general" | "sensitive" | "adult" | "unrated";

export type ListingHeader = {
  hash: string;
  storeId: string;
  title: string;
  category: string;
  priceSats: number;
  shipFrom: string;
  kind: ListingKind;
  acceptsModerator: boolean;
  /** Monero is on at this node and the store's signed record takes it. */
  acceptsXmr: boolean;
  /** Lightning runs at this node and the store's signed record takes it. */
  acceptsLightning: boolean;
  description?: string;
  seller: string;
  stock: number;
  listedOn: number;
  product: string;
  shippingSats: number;
  freeOverSats: number;
  contentRating: ContentRating;
  contentTags: string[];
  /** Set on listings a seller made in the app (signed version 6). */
  listingId: string | null;
  /** SHA-256 hashes of the listing's images, served at `/media/{hash}`. */
  images: string[];
  /** The seller's fiat price; `priceSats` is that price at the rate the day it was signed. */
  price: import("./records.ts").FiatPrice | null;
  /** Version 7 terms; absent or null on older listings. */
  commerce?: import("./commerce.ts").Commerce | null;
};

export type Moderator = {
  name: string;
  id: string;
};

export type ShopCatalog = {
  /** The first store, with the strictest rating of all stores. */
  store: {
    id: string;
    name: string;
    region: string;
    contentRating: ContentRating;
  };
  /** Every store whose signed record verified. */
  stores: StoreSummary[];
  moderator: Moderator | null;
  /** Only listings whose signed record verified and whose store is in `stores`. */
  listings: ListingHeader[];
  /** Listings dropped because their record was missing, did not verify, or named an unknown store. */
  rejected: number;
  /** Why this node refuses Lightning (mainnet), or `null`. No listing takes it then. */
  lightningBlocked: string | null;
};

export type ShipTo = {
  name: string;
  street: string;
  city: string;
  region: string;
  postalCode: string;
};

export type CheckoutLine = {
  listingHash: string;
  quantity: number;
  /** A version 7 listing's variant id. */
  variant?: string;
};

export type CheckoutRequest = {
  listingHash?: string;
  quantity?: number;
  lines?: CheckoutLine[];
  settlement: "direct" | "escrow";
  shipTo?: ShipTo & { street2?: string; country?: string };
  deliveryNote?: string;
  addressId?: string;
  addressLabel?: string;
};

export type SavedAddress = {
  id: string;
  label: string;
  name: string;
  street: string;
  street2: string;
  city: string;
  region: string;
  postalCode: string;
  country: string;
  default: boolean;
};

export type AddressInput = {
  label: string;
  name: string;
  street: string;
  street2?: string;
  city: string;
  region: string;
  postalCode: string;
  country: string;
  default?: boolean;
};

export type BuyerLine = {
  listingHash: string;
  title: string;
  quantity: number;
  priceSats: number;
  /** "Navy / M" for a variant. */
  variantLabel?: string;
};

export type NetworkName = "testnet4" | "mainnet";
export type OrderTracking = {
  network: NetworkName | null;
  receivedSats: number;
  confirmations: number;
  txids: string[];
  payTxid: string | null;
  paying: boolean;
  payError: string | null;
};

export type BuyerOrder = OrderTracking & {
  id: string;
  checkoutId: string;
  created: number;
  seller: string;
  storeId: string;
  lines: BuyerLine[];
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  settlement: "direct" | "escrow";
  addressLabel: string;
  status: string;
  payment: DirectPayment | null;
  escrow: EscrowReceipt | null;
  /** An order to a store on another node: delivery and the seller's replies. */
  remote: RemoteOrder | null;
};

export type Fulfillment = { carrier: string; tracking: string; note: string; content: string };

export type RemoteOrder = {
  delivered: boolean;
  error: string | null;
  reason: string | null;
  sellerSawPayment: boolean;
  sellerCompleted: boolean;
  fulfillment: Fulfillment | null;
  /** The seller sold out before this order was paid, and owes a refund. */
  oversold: boolean;
  /** Signed receipts kept for reviewing this order. */
  receipts: number;
  /** The buyer's review of this order, waiting to publish or published. */
  review: OrderReview | null;
};

export type OrderReview = { rating: number; text: string; published: boolean; sendAfter: number; writtenOn: number | null };

export type DirectPayment = {
  address: string;
  amountSats: number | null;
  network: NetworkName | null;
  silentPaymentCode: string;
  lightning: string;
  onchain: string;
  /** The order's BOLT 11 invoice when it is paid over Lightning. */
  ln: LnInvoice | null;
};

export type EscrowReceipt = {
  fundingSats: number;
  timeoutHeight: number;
  address: string;
  outpoint: string;
  signaturesRequired: number;
};

export type SellerOrder = OrderTracking & {
  orderId: string;
  seller: string;
  shipFrom: string;
  lines: BuyerLine[];
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  settlement: "direct" | "escrow";
  addressLabel: string;
  status: string;
  payment: DirectPayment | null;
  escrow: EscrowReceipt | null;
};

export type CheckoutReceipt = {
  checkoutId: string;
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  orders: SellerOrder[];
};

export type OrderReceipt = {
  letterId: string;
  title: string;
  quantity: number;
  priceSats: number;
  totalSats: number;
  settlement: "direct" | "escrow";
  sealed: boolean;
  storeName: string;
  orderId: string;
  seller: string;
  addressLabel: string;
  lines: BuyerLine[];
  payment: DirectPayment | null;
  escrow: EscrowReceipt | null;
};

export async function fetchCatalog(): Promise<ShopCatalog> {
  const response = await request("/catalog");
  return parseCatalog(await readJson(response));
}

export async function paired(path: string, init?: RequestInit): Promise<unknown> {
  const response = await pairedResponse(path, init);
  if (init?.method === "DELETE") {
    return readJson(response);
  }
  return readJson(response);
}

/// A request only the paired client may make, as the raw response.
export async function pairedResponse(path: string, init?: RequestInit): Promise<Response> {
  const token = await pairingToken();
  return request(path, {
    ...init,
    headers: {
      ...(init?.headers ?? {}),
      [PAIR_HEADER]: token,
    },
  });
}

/// A paired request to this app's own node, even while a home node is the target.
export async function ownNodeJson(path: string, init?: RequestInit): Promise<unknown> {
  const token = await pairingToken();
  let response: Response;
  try {
    response = await fetch(`${NODE_ORIGIN}${path}`, { cache: "no-store", ...init, headers: { ...(init?.headers ?? {}), [PAIR_HEADER]: token } });
  } catch {
    throw new NodeOfflineError();
  }
  if (!response.ok) {
    const { message, code, retryAfterSecs } = await errorDetail(response);
    throw new NodeApiError(response.status, message, code, retryAfterSecs);
  }
  return readJson(response);
}

/// `GET` a paired JSON endpoint.
export async function pairedJson(path: string): Promise<unknown> {
  return paired(path);
}

export async function fetchAddresses(): Promise<SavedAddress[]> {
  const body = recordOf(await paired("/addresses"), "The address book could not be read.");
  if (!Array.isArray(body.addresses)) {
    return [];
  }
  return body.addresses.map(parseAddress);
}

export async function createAddress(input: AddressInput): Promise<SavedAddress> {
  return parseAddress(await paired("/addresses", { method: "POST", body: JSON.stringify(input), headers: { "Content-Type": "application/json" } }));
}

export async function updateAddress(id: string, input: AddressInput): Promise<SavedAddress> {
  return parseAddress(
    await paired(`/addresses/${id}`, {
      method: "PUT",
      body: JSON.stringify(input),
      headers: { "Content-Type": "application/json" },
    }),
  );
}

export async function deleteAddress(id: string): Promise<void> {
  await paired(`/addresses/${id}`, { method: "DELETE" });
}

export async function fetchOrders(): Promise<BuyerOrder[]> {
  const body = recordOf(await paired("/orders"), "The orders could not be read.");
  if (!Array.isArray(body.orders)) {
    return [];
  }
  return body.orders.map(parseBuyerOrder);
}

export async function fetchOrder(id: string): Promise<BuyerOrder> {
  return parseBuyerOrder(await paired(`/orders/${id}`));
}

export async function placeCheckout(order: {
  addressId?: string;
  addressLabel?: string;
  shipTo?: ShipTo & { street2?: string; country?: string };
  deliveryNote?: string;
  packages: { settlement: "direct" | "escrow"; rail?: "btc" | "xmr" | "ln"; lines: CheckoutLine[]; service?: number | "pickup"; coupon?: string }[];
}): Promise<CheckoutReceipt> {
  const token = await pairingToken();
  const response = await request("/checkout", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      [PAIR_HEADER]: token,
    },
    body: JSON.stringify(order),
  });
  return parseCheckout(await readJson(response));
}

export async function placeOrder(order: CheckoutRequest): Promise<OrderReceipt> {
  const token = await pairingToken();
  const response = await request("/orders", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      [PAIR_HEADER]: token,
    },
    body: JSON.stringify(order),
  });
  return parseReceipt(await readJson(response));
}

/// Sign a store or one of its listings again with a new self-label.
/// A listing gets a new hash; the returned catalog already carries it.
/// `storeKey` picks the store when `target` is "store"; without it, the first store.
export async function setContentLabel(label: {
  target: "store" | string;
  storeKey?: string;
  rating: Exclude<ContentRating, "unrated">;
  tags: string[];
}): Promise<{ hash: string | null; catalog: ShopCatalog }> {
  const body = recordOf(
    await paired("/content-labels", { method: "POST", body: JSON.stringify(label), headers: { "Content-Type": "application/json" } }),
    "The label could not be saved.",
  );
  return { hash: typeof body.hash === "string" ? body.hash : null, catalog: parseCatalog(body.catalog) };
}

export type FollowedLabeler = { key: string; url: string | null; name: string | null; sequence: number | null; error: string | null };
export type LabelerState = { labelers: FollowedLabeler[]; samples: { key: string; name: string }[]; labels: LabelerLabel[] };

/// Labelers this node follows and their verified labels. Only the paired client sees the list.
export async function fetchLabelers(): Promise<LabelerState> {
  return parseLabelers(await paired("/labelers"));
}

export async function setLabelers(labelers: { key: string; url?: string | null }[]): Promise<LabelerState> {
  return parseLabelers(
    await paired("/labelers", { method: "PUT", body: JSON.stringify({ labelers }), headers: { "Content-Type": "application/json" } }),
  );
}

function parseLabelers(value: unknown): LabelerState {
  const body = recordOf(value, "The labelers could not be read.");
  const list = (items: unknown) => (Array.isArray(items) ? items.filter((item): item is Record<string, unknown> => !!item && typeof item === "object") : []);
  const text = (item: unknown) => (typeof item === "string" ? item : null);
  return {
    labelers: list(body.labelers).map((item) => ({
      key: text(item.key) ?? "",
      url: text(item.url),
      name: text(item.name),
      sequence: typeof item.sequence === "number" ? item.sequence : null,
      error: text(item.error),
    })),
    samples: list(body.samples).map((item) => ({ key: text(item.key) ?? "", name: text(item.name) ?? "" })),
    labels: list(body.labels).flatMap((item): LabelerLabel[] => {
      const rating = item.rating;
      const targetKind = item.targetKind;
      if ((rating !== "general" && rating !== "sensitive" && rating !== "adult") || (targetKind !== "listing" && targetKind !== "store")) return [];
      return [{
        labeler: text(item.labeler) ?? "",
        labelerName: text(item.labelerName) ?? "a labeler",
        targetKind,
        target: text(item.target) ?? "",
        rating,
        tags: Array.isArray(item.tags) ? item.tags.filter((tag): tag is string => typeof tag === "string") : [],
        reason: text(item.reason) ?? "",
      }];
    }),
  };
}

let pairing: Promise<string> | null = null;

let localShopSession: Promise<boolean> | null = null;

/// The localhost cookie is enough. The pairing token stays out of this page.
function localShopSignedIn(): Promise<boolean> {
  if (!localShopSession) {
    const origin = typeof location === "undefined" ? "" : location.origin;
    localShopSession = fetch(`${origin}/browser-session`, { cache: "no-store" })
      .then((response) => response.ok)
      .catch(() => false);
  }
  return localShopSession;
}

async function pairingToken(): Promise<string> {
  if (servedByHomeNode() || homeFetch()) {
    return "";
  }
  if (typeof DESKTOP_TOKEN === "string" && TOKEN_PATTERN.test(DESKTOP_TOKEN)) {
    return DESKTOP_TOKEN;
  }
  if (servedByLocalShop() && (await localShopSignedIn())) {
    return "";
  }
  const stored = tokenStore()?.getItem(TOKEN_KEY);
  if (stored && TOKEN_PATTERN.test(stored)) {
    return stored;
  }
  pairing ??= pairWithCode().finally(() => {
    pairing = null;
  });
  return pairing;
}

function tokenStore(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function forgetToken(): void {
  tokenStore()?.removeItem(TOKEN_KEY);
}

/// Ask the node to print a code, then trade the code the user types for the token.
async function pairWithCode(): Promise<string> {
  const asked = await postPair({});
  if (asked.status === 403) {
    throw new Error(asked.error ?? "This node does not pair browser shops.");
  }
  const ask = (globalThis as { prompt?: (message: string) => string | null }).prompt;
  if (typeof ask !== "function") {
    throw new Error("Open the shop in a browser to pair it with your node.");
  }
  const code = ask("Type the pairing code shown in your node's terminal.")?.trim();
  if (!code) {
    throw new Error("The shop is not paired with this node.");
  }
  const traded = await postPair({ code });
  if (typeof traded.token !== "string" || !TOKEN_PATTERN.test(traded.token)) {
    throw new Error(traded.error ?? "The shop is not paired with this node.");
  }
  tokenStore()?.setItem(TOKEN_KEY, traded.token);
  return traded.token;
}

async function postPair(body: { code?: string }): Promise<{ status: number; token?: unknown; error?: string }> {
  let response: Response;
  try {
    response = await fetch(`${NODE_ORIGIN}/pair`, {
      method: "POST",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    throw new NodeOfflineError();
  }
  const value = recordOf(await readJson(response), OFFLINE_MESSAGE);
  return {
    status: response.status,
    token: value.token,
    error: typeof value.error === "string" ? value.error : undefined,
  };
}

export type CurrencyCode = "USD" | "EUR" | "GBP" | "JPY";

export type RateTable = Partial<Record<CurrencyCode, number>>;

const CURRENCIES: readonly CurrencyCode[] = ["USD", "EUR", "GBP", "JPY"];

/// Bitcoin prices for the estimate beside a listing. A failure here is not
/// the shop being offline: the bitcoin amount still stands on its own.
export async function fetchRates(): Promise<RateTable> {
  try {
    const response = await nodeFetch("/rates", { cache: "no-store" });
    if (!response.ok) {
      return {};
    }
    return parseRates(await response.json());
  } catch {
    return {};
  }
}

function parseRates(value: unknown): RateTable {
  if (!value || typeof value !== "object") {
    return {};
  }
  const currencies = (value as { currencies?: unknown }).currencies;
  if (!currencies || typeof currencies !== "object") {
    return {};
  }
  const record = currencies as Record<string, unknown>;
  const rates: RateTable = {};
  for (const code of CURRENCIES) {
    const price = record[code];
    if (typeof price === "number" && Number.isFinite(price) && price > 0) {
      rates[code] = price;
    }
  }
  return rates;
}

export class NodeApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly retryAfterSecs?: number;
  constructor(status: number, message: string, code?: string, retryAfterSecs?: number) {
    super(message);
    this.name = "NodeApiError";
    this.status = status;
    this.code = code;
    this.retryAfterSecs = retryAfterSecs;
  }
}

// The proxy to a home node is paired even for endpoints that are open locally.
async function nodeFetch(path: string, init: RequestInit): Promise<Response> {
  if (servedByHomeNode()) return signedFetch(path, init);
  const native = homeFetch();
  if (native) return native(path, init);
  const prefix = targetPrefix();
  if (!prefix) return fetch(`${NODE_ORIGIN}${path}`, init);
  const headers = new Headers(init.headers);
  headers.set(PAIR_HEADER, await pairingToken());
  return fetch(`${NODE_ORIGIN}${prefix}${path}`, { ...init, headers });
}

async function request(path: string, init?: RequestInit): Promise<Response> {
  let response: Response;
  try {
    response = await nodeFetch(path, { cache: "no-store", ...init });
  } catch {
    throw new NodeOfflineError();
  }
  if (!response.ok) {
    if (response.status === 401 || response.status === 403) {
      forgetToken();
    }
    const { message, code, retryAfterSecs } = await errorDetail(response);
    throw new NodeApiError(response.status, message, code, retryAfterSecs);
  }
  return response;
}

async function readJson(response: Response): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    throw new Error("The shop sent a response the page could not read.");
  }
}

async function errorDetail(response: Response): Promise<{ message: string; code?: string; retryAfterSecs?: number }> {
  let body: { error?: unknown; code?: unknown; retryAfterSecs?: unknown } = {};
  try {
    body = (await response.json()) as typeof body;
  } catch {
    // The status line is enough when the body is not JSON.
  }
  const code = typeof body.code === "string" ? body.code : undefined;
  const retryAfterSecs =
    typeof body.retryAfterSecs === "number" && Number.isFinite(body.retryAfterSecs) ? body.retryAfterSecs : undefined;
  if (typeof body.error === "string" && body.error.trim()) {
    return { message: body.error, code, retryAfterSecs };
  }
  if (response.status === 404) {
    return { message: "That listing is not in this shop.", code };
  }
  return { message: "The shop can't be reached. The node returned an error.", code };
}

const RATING_RANK: Record<ContentRating, number> = { general: 0, unrated: 1, sensitive: 2, adult: 3 };

/// Every listing and store is checked against its signed record here; the
/// page never sees a header the store key did not sign.
export function parseCatalog(value: unknown): ShopCatalog {
  const record = recordOf(value, "The shop sent a catalog the page could not read.");
  if (!Array.isArray(record.listings)) {
    throw new Error("The shop sent a catalog the page could not read.");
  }
  const stores = (Array.isArray(record.stores) ? record.stores : [])
    .map(verifiedStore)
    .filter((store): store is StoreSummary => store !== null);
  const { listings, rejected } = verifiedListings(
    record.listings,
    new Set(stores.map((store) => store.key)),
    new Set(stores.filter((store) => store.acceptsXmr).map((store) => store.key)),
    lnStoreKeys(stores, record.lightning),
  );
  const first = stores[0];
  const strictest = stores.reduce<ContentRating>(
    (rating, store) => (RATING_RANK[store.contentRating] > RATING_RANK[rating] ? store.contentRating : rating),
    "general",
  );
  return {
    store: {
      id: first?.key ?? "",
      name: first?.name ?? "",
      region: first?.region ?? "",
      contentRating: first ? strictest : "unrated",
    },
    stores,
    moderator: parseModerator(record.moderator),
    listings,
    rejected,
    lightningBlocked: lnBlockedReason(record.lightning),
  };
}

/// The signed rating of the store that sold `listing`.
export function storeRatingOf(catalog: ShopCatalog, listing: ListingHeader): ContentRating {
  return catalog.stores.find((store) => store.key === listing.storeId)?.contentRating ?? "unrated";
}

function parseAddress(value: unknown): SavedAddress {
  const address = recordOf(value, "The address book could not be read.");
  return {
    id: requiredString(address.id),
    label: requiredString(address.label),
    name: requiredString(address.name),
    street: requiredString(address.street),
    street2: typeof address.street2 === "string" ? address.street2 : "",
    city: requiredString(address.city),
    region: requiredString(address.region),
    postalCode: requiredString(address.postalCode),
    country: requiredString(address.country),
    default: address.default === true,
  };
}

export function parseBuyerOrder(value: unknown): BuyerOrder {
  const order = recordOf(value, "The orders could not be read.");
  const settlement = order.settlement;
  if (settlement !== "direct" && settlement !== "escrow") {
    throw new Error("The orders could not be read.");
  }
  return {
    id: requiredString(order.id),
    checkoutId: typeof order.checkoutId === "string" ? order.checkoutId : "",
    created: requiredNumber(order.created),
    seller: requiredString(order.seller),
    storeId: requiredString(order.storeId),
    itemsSats: typeof order.itemsSats === "number" ? order.itemsSats : 0,
    shippingSats: typeof order.shippingSats === "number" ? order.shippingSats : 0,
    lines: parseLines(order.lines, {
      listingHash: "unknown",
      title: "Item",
      quantity: 1,
      priceSats: requiredNumber(order.totalSats),
    }),
    totalSats: requiredNumber(order.totalSats),
    settlement,
    addressLabel: typeof order.addressLabel === "string" ? order.addressLabel : "",
    ...parseTracking(order),
    status: typeof order.status === "string" && order.status.trim() ? order.status : "Awaiting payment",
    payment: settlement === "direct" && order.payment != null ? parsePayment(order.payment) : null,
    escrow: settlement === "escrow" ? parseEscrow(order.escrow) : null,
    remote: parseRemote(order.remote),
  };
}

function parseRemote(value: unknown): RemoteOrder | null {
  if (!value || typeof value !== "object") return null;
  const remote = value as Record<string, unknown>;
  const text = (v: unknown) => (typeof v === "string" ? v : "");
  const fulfillment = remote.fulfillment && typeof remote.fulfillment === "object" ? (remote.fulfillment as Record<string, unknown>) : null;
  return {
    delivered: remote.delivered === true,
    error: typeof remote.error === "string" ? remote.error : null,
    reason: typeof remote.reason === "string" ? remote.reason : null,
    sellerSawPayment: remote.sellerSawPayment === true,
    sellerCompleted: remote.sellerCompleted === true,
    fulfillment: fulfillment
      ? { carrier: text(fulfillment.carrier), tracking: text(fulfillment.tracking), note: text(fulfillment.note), content: text(fulfillment.content) }
      : null,
    oversold: Array.isArray(remote.oversold) && remote.oversold.length > 0,
    receipts: Array.isArray(remote.receipts) ? remote.receipts.length : 0,
    review: parseOrderReview(remote.review),
  };
}

function parseOrderReview(value: unknown): OrderReview | null {
  if (!value || typeof value !== "object") return null;
  const review = value as Record<string, unknown>;
  const number = (v: unknown) => (typeof v === "number" ? v : 0);
  return { rating: number(review.rating), text: typeof review.text === "string" ? review.text : "", published: review.published === true, sendAfter: number(review.sendAfter), writtenOn: typeof review.writtenOn === "number" ? review.writtenOn : null };
}

function parseModerator(value: unknown): Moderator | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const record = value as Record<string, unknown>;
  const name = record.name;
  const id = record.id;
  if (typeof name !== "string" || typeof id !== "string" || !name.trim() || !id.trim()) {
    return null;
  }
  return { name: name.trim(), id: id.trim() };
}

function parseCheckout(value: unknown): CheckoutReceipt {
  const receipt = recordOf(value, "The shop accepted the order but the page could not read the receipt.");
  if (receipt.sealed !== true || !Array.isArray(receipt.orders) || receipt.orders.length === 0) {
    throw new Error("The shop accepted the order but the page could not read the receipt.");
  }
  return {
    checkoutId: requiredString(receipt.checkoutId),
    itemsSats: requiredNumber(receipt.itemsSats),
    shippingSats: requiredNumber(receipt.shippingSats),
    totalSats: requiredNumber(receipt.totalSats),
    orders: receipt.orders.map(parseSellerOrder),
  };
}

function parseSellerOrder(value: unknown): SellerOrder {
  const order = recordOf(value, "The shop accepted the order but the page could not read the receipt.");
  const settlement = order.settlement;
  if (settlement !== "direct" && settlement !== "escrow") {
    throw new Error("The shop accepted the order but the page could not read the receipt.");
  }
  return {
    orderId: requiredString(order.orderId),
    seller: requiredString(order.seller),
    shipFrom: typeof order.shipFrom === "string" ? order.shipFrom : "",
    lines: parseLines(order.lines, {
      listingHash: "unknown",
      title: "Item",
      quantity: 1,
      priceSats: requiredNumber(order.totalSats),
    }),
    itemsSats: requiredNumber(order.itemsSats),
    shippingSats: requiredNumber(order.shippingSats),
    totalSats: requiredNumber(order.totalSats),
    settlement,
    addressLabel: typeof order.addressLabel === "string" ? order.addressLabel : "",
    ...parseTracking(order),
    status: typeof order.status === "string" && order.status.trim() ? order.status : "Awaiting payment",
    payment: settlement === "direct" && order.payment != null ? parsePayment(order.payment) : null,
    escrow: settlement === "escrow" ? parseEscrow(order.escrow) : null,
  };
}

function parseReceipt(value: unknown): OrderReceipt {
  const receipt = recordOf(value, "The shop accepted the order but the page could not read the receipt.");
  const settlement = receipt.settlement;
  if (settlement !== "direct" && settlement !== "escrow") {
    throw new Error("The shop accepted the order but the page could not read the receipt.");
  }
  const payment = settlement === "direct" ? parsePayment(receipt.payment) : null;
  const escrow = settlement === "escrow" ? parseEscrow(receipt.escrow) : null;
  if (receipt.sealed !== true) {
    throw new Error("The shop accepted the order but the page could not read the receipt.");
  }
  return {
    letterId: requiredString(receipt.letterId),
    title: requiredString(receipt.title),
    quantity: requiredNumber(receipt.quantity),
    priceSats: requiredNumber(receipt.priceSats),
    totalSats: requiredNumber(receipt.totalSats),
    settlement,
    sealed: true,
    storeName: requiredString(receipt.storeName),
    orderId: typeof receipt.orderId === "string" ? receipt.orderId : requiredString(receipt.letterId),
    seller: typeof receipt.seller === "string" ? receipt.seller : requiredString(receipt.storeName),
    addressLabel: typeof receipt.addressLabel === "string" ? receipt.addressLabel : "",
    lines: parseLines(receipt.lines, {
      listingHash: requiredString(receipt.listingHash),
      title: requiredString(receipt.title),
      quantity: requiredNumber(receipt.quantity),
      priceSats: requiredNumber(receipt.priceSats),
    }),
    payment,
    escrow,
  };
}

function parseLines(value: unknown, fallback: BuyerLine): BuyerLine[] {
  if (!Array.isArray(value) || value.length === 0) {
    return [fallback];
  }
  return value.map((item) => {
    const line = recordOf(item, "The shop accepted the order but the page could not read the receipt.");
    return {
      listingHash: requiredString(line.listingHash),
      title: requiredString(line.title),
      quantity: requiredNumber(line.quantity),
      priceSats: requiredNumber(line.priceSats),
      ...(typeof line.variantLabel === "string" ? { variantLabel: line.variantLabel } : {}),
    };
  });
}

function parsePayment(value: unknown): DirectPayment {
  const payment = recordOf(value, "The shop accepted the order but the page could not read the receipt.");
  return {
    address: typeof payment.address === "string" ? payment.address : "",
    amountSats: typeof payment.amountSats === "number" ? requiredNumber(payment.amountSats) : null,
    network: parseNetworkName(payment.network),
    silentPaymentCode: typeof payment.silentPaymentCode === "string" ? payment.silentPaymentCode : "",
    lightning: typeof payment.lightning === "string" ? payment.lightning : "",
    onchain: typeof payment.onchain === "string" ? payment.onchain : "",
    ln: parseLnInvoice(payment),
  };
}

function parseEscrow(value: unknown): EscrowReceipt | null {
  if (value == null) return null;
  const escrow = recordOf(value, "The shop sent unreadable escrow terms.");
  // Awaiting vendor terms has no funding address yet. It stays read-only.
  return {
    fundingSats: typeof escrow.fundingSats === "number" ? requiredNumber(escrow.fundingSats) : 0,
    timeoutHeight: typeof escrow.timeoutHeight === "number" ? requiredNumber(escrow.timeoutHeight) : 0,
    address: typeof escrow.address === "string" ? escrow.address : "",
    outpoint: typeof escrow.outpoint === "string" ? escrow.outpoint : "",
    signaturesRequired: typeof escrow.signaturesRequired === "number" ? requiredNumber(escrow.signaturesRequired) : 0,
  };
}

function recordOf(value: unknown, message: string): Record<string, unknown> {
  if (!value || typeof value !== "object") {
    throw new Error(message);
  }
  return value as Record<string, unknown>;
}

function requiredString(value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error("The shop sent a record the page could not read.");
  }
  return value;
}

function requiredNumber(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error("The shop sent a record the page could not read.");
  }
  return value;
}

export function parseNetworkName(value: unknown): NetworkName | null {
  return value === "testnet4" || value === "mainnet" ? value : null;
}
function parseTracking(order: Record<string, unknown>): OrderTracking {
  return {
    network: parseNetworkName(order.network),
    receivedSats: typeof order.receivedSats === "number" ? requiredNumber(order.receivedSats) : 0,
    confirmations: typeof order.confirmations === "number" ? requiredNumber(order.confirmations) : 0,
    txids: Array.isArray(order.txids) ? order.txids.filter((id): id is string => typeof id === "string" && /^[0-9a-f]{64}$/i.test(id)) : [],
    payTxid: typeof order.payTxid === "string" && /^[0-9a-f]{64}$/i.test(order.payTxid) ? order.payTxid : null,
    paying: order.paying === true,
    payError: typeof order.payError === "string" ? order.payError : null,
  };
}
