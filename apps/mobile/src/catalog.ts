import type { LabelerLabel } from "./safety/filter";

// The shop dials only this origin. A query parameter, settings field, seed,
// peer, or vendor address cannot point it anywhere else. Paired requests send
// the node's pairing token as X-Ob-Pair. The desktop app starts its own node
// on a free loopback port and sets __OB_NODE_ORIGIN__ and __OB_PAIR_TOKEN__
// before this script runs; only a 127.0.0.1 origin is taken. In a browser the
// node gives the token only for a one-time code printed in its terminal.
const DESKTOP_NODE_ORIGIN = (globalThis as { __OB_NODE_ORIGIN__?: unknown }).__OB_NODE_ORIGIN__;
export const NODE_ORIGIN =
  typeof DESKTOP_NODE_ORIGIN === "string" && /^http:\/\/127\.0\.0\.1:\d{1,5}$/.test(DESKTOP_NODE_ORIGIN)
    ? DESKTOP_NODE_ORIGIN
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
  description?: string;
  seller: string;
  stock: number;
  listedOn: number;
  product: string;
  shippingSats: number;
  freeOverSats: number;
  contentRating: ContentRating;
  contentTags: string[];
};

export type Moderator = {
  name: string;
  id: string;
};

export type ShopCatalog = {
  store: {
    id: string;
    name: string;
    region: string;
    contentRating: ContentRating;
  };
  moderator: Moderator | null;
  listings: ListingHeader[];
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
};

export type DirectPayment = {
  address: string;
  amountSats: number | null;
  network: NetworkName | null;
  silentPaymentCode: string;
  lightning: string;
  onchain: string;
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
  const token = await pairingToken();
  const response = await request(path, {
    ...init,
    headers: {
      ...(init?.headers ?? {}),
      [PAIR_HEADER]: token,
    },
  });
  if (init?.method === "DELETE") {
    return readJson(response);
  }
  return readJson(response);
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
  packages: { settlement: "direct" | "escrow"; lines: CheckoutLine[] }[];
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

/// Sign the store or one of its listings again with a new self-label.
/// A listing gets a new hash; the returned catalog already carries it.
export async function setContentLabel(label: {
  target: "store" | string;
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

async function pairingToken(): Promise<string> {
  if (typeof DESKTOP_TOKEN === "string" && TOKEN_PATTERN.test(DESKTOP_TOKEN)) {
    return DESKTOP_TOKEN;
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
    const response = await fetch(`${NODE_ORIGIN}/rates`, { cache: "no-store" });
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
  constructor(public readonly status: number, message: string) { super(message); this.name = "NodeApiError"; }
}

async function request(path: string, init?: RequestInit): Promise<Response> {
  let response: Response;
  try {
    response = await fetch(`${NODE_ORIGIN}${path}`, { cache: "no-store", ...init });
  } catch {
    throw new NodeOfflineError();
  }
  if (!response.ok) {
    if (response.status === 401 || response.status === 403) {
      forgetToken();
    }
    throw new NodeApiError(response.status, await errorMessage(response));
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

async function errorMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { error?: unknown };
    if (typeof body.error === "string" && body.error.trim()) {
      return body.error;
    }
  } catch {
    // The status line is enough when the body is not JSON.
  }
  if (response.status === 404) {
    return "That listing is not in this shop.";
  }
  return "The shop can't be reached. The node returned an error.";
}

function parseCatalog(value: unknown): ShopCatalog {
  const record = recordOf(value, "The shop sent a catalog the page could not read.");
  const store = recordOf(record.store, "The shop sent a catalog the page could not read.");
  const listings = record.listings;
  if (!Array.isArray(listings)) {
    throw new Error("The shop sent a catalog the page could not read.");
  }
  return {
    store: {
      id: requiredString(store.id),
      name: requiredString(store.name),
      region: requiredString(store.region),
      contentRating: parseRating(store.contentRating),
    },
    moderator: parseModerator(record.moderator),
    listings: listings.map(parseListing),
  };
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
    payment: settlement === "direct" ? parsePayment(order.payment) : null,
    escrow: settlement === "escrow" ? parseEscrow(order.escrow) : null,
  };
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

function parseListing(value: unknown): ListingHeader {
  const listing = recordOf(value, "The shop sent a listing the page could not read.");
  const kind = listing.kind;
  if (kind !== "physical" && kind !== "digital") {
    throw new Error("The shop sent a listing the page could not read.");
  }
  const description =
    typeof listing.description === "string" && listing.description.trim()
      ? listing.description.trim()
      : undefined;
  return {
    hash: requiredString(listing.hash),
    storeId: requiredString(listing.storeId),
    title: requiredString(listing.title),
    category: requiredString(listing.category),
    priceSats: requiredNumber(listing.priceSats),
    shipFrom: requiredString(listing.shipFrom),
    kind,
    acceptsModerator: listing.acceptsModerator === true,
    description,
    seller: requiredString(listing.seller),
    stock: requiredNumber(listing.stock),
    listedOn: requiredNumber(listing.listedOn),
    product: typeof listing.product === "string" ? listing.product : "",
    shippingSats: listing.shippingSats === undefined ? 0 : requiredNumber(listing.shippingSats),
    freeOverSats: listing.freeOverSats === undefined ? 0 : requiredNumber(listing.freeOverSats),
    contentRating: parseRating(listing.contentRating),
    contentTags: Array.isArray(listing.contentTags)
      ? listing.contentTags.filter((tag): tag is string => typeof tag === "string")
      : [],
  };
}

// A node that predates labels sends nothing: unrated. A rating this page
// does not know yet is treated as the most restricted one.
function parseRating(value: unknown): ContentRating {
  if (value === undefined || value === null) return "unrated";
  if (value === "general" || value === "sensitive" || value === "adult" || value === "unrated") return value;
  return "adult";
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
    payment: settlement === "direct" ? parsePayment(order.payment) : null,
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
