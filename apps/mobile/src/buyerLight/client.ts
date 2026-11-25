// Signed-app-code buyer-only light client.
//
// Communicates directly with a seller's public endpoints without requiring
// a 24/7 buyer Tor mailbox or a paired home node:
// - `GET {seller}/public/store`: verifies signed store & listing records locally
// - `POST {seller}/mail`: sends sealed pull-mode `OBOD` order (or chat/complete)
// - `POST {seller}/mail/pull`: authenticated pull of sealed order status/tracking/chat/return
//
// Web/Tor Browser stays strictly browse-only: this client is only mounted in
// signed native app code (`Platform.OS !== "web"`).

import { bytesToHex, hexToBytes, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import type { ListingHeader, ShopCatalog } from "../catalog.ts";
import { catalogFromPublic, DIRECT_GET, NOT_A_STORE, storeBase } from "../directStore.ts";
import { verifyListingRecord, verifyStoreRecord } from "../records.ts";
import { applySellerUpdate, type LightOrderRecord } from "./contracts.ts";
import { deriveOrderKeys, type BuyerSeed } from "./keys.ts";
import { assertEscrowAllowed, type PaymentRailKind } from "./escrowVerify.ts";
import { openSealedReply, sealMessageLetter, sealOrderLetter } from "./tormail.ts";

export type LightFetcher = (url: string, init?: RequestInit) => Promise<Response>;

const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;

export type VerifiedPublicStore = {
  readonly url: string;
  readonly catalog: ShopCatalog;
  readonly storeRecordHex: string;
  readonly listingRecordsByHash: Record<string, string>;
  readonly storeMailboxHex: string;
};

/**
 * Fetch and verify a seller's `/public/store`, retaining the raw signed hex
 * records so checkout can persist them into the buyer's local contract store.
 */
export async function fetchVerifiedStore(
  pastedUrl: string,
  fetcher: LightFetcher = globalThis.fetch,
): Promise<VerifiedPublicStore> {
  const url = storeBase(pastedUrl);
  if (!url) throw new Error(NOT_A_STORE);
  const response = await fetcher(`${url}/public/store`, DIRECT_GET);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > MAX_RESPONSE_BYTES) throw new Error("That store's answer is too large.");
  if (!response.ok) {
    throw new Error("That store did not answer or is not selling right now.");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    throw new Error("That store's answer could not be read.");
  }
  const catalog = catalogFromPublic(parsed);
  const raw = parsed as { store: string; listings: string[] };
  const verifiedStore = verifyStoreRecord(raw.store);
  if (!verifiedStore?.details?.mailboxKey) {
    throw new Error("This store has no signed mailbox key for sealed orders.");
  }
  const listingRecordsByHash: Record<string, string> = {};
  for (const hex of raw.listings) {
    const rec = verifyListingRecord(hex);
    if (rec) listingRecordsByHash[rec.hash] = hex;
  }
  return {
    url,
    catalog,
    storeRecordHex: raw.store,
    listingRecordsByHash,
    storeMailboxHex: verifiedStore.details.mailboxKey,
  };
}

/**
 * Seal and place a pull-mode order directly with a seller's `POST /mail`.
 *
 * Returns a locally retained `LightOrderRecord` with the signed store and
 * listing records attached, plus immediate verified payment terms if the
 * seller returned a sealed reply synchronously.
 */
export async function submitLightOrder(params: {
  seed: BuyerSeed;
  store: VerifiedPublicStore;
  listing: ListingHeader;
  quantity: number;
  shipTo: string;
  settlement: "direct" | "escrow";
  rail?: PaymentRailKind;
  checkoutId?: string;
  fetcher?: LightFetcher;
}): Promise<LightOrderRecord> {
  const rail: PaymentRailKind = params.rail ?? "btc";
  if (params.settlement === "escrow") {
    assertEscrowAllowed(params.seed.network, rail);
  }
  if (!Number.isInteger(params.quantity) || params.quantity < 1 || params.quantity > 99) {
    throw new Error("Enter a quantity between 1 and 99.");
  }
  if (!params.shipTo.trim()) {
    throw new Error(params.listing.kind === "physical" ? "Enter a shipping address." : "Enter a delivery note.");
  }
  const listingRecordHex = params.store.listingRecordsByHash[params.listing.hash];
  if (!listingRecordHex) {
    throw new Error("That listing's signed record is missing.");
  }

  const checkoutId = params.checkoutId ?? bytesToHex(randomBytes(16));
  const orderKeys = deriveOrderKeys(params.seed, checkoutId, params.listing.storeId);
  const itemsSats = params.listing.priceSats * params.quantity;
  const shippingSats =
    params.listing.kind === "physical"
      ? params.listing.freeOverSats > 0 && itemsSats >= params.listing.freeOverSats
        ? 0
        : params.listing.shippingSats
      : 0;
  const estimateSats = itemsSats + shippingSats;

  const options: Record<string, string> = {
    settlement: params.settlement,
    shipping: String(shippingSats),
    replyMode: "pull",
  };
  if (rail !== "btc") {
    options.rail = rail;
  }

  const sealedOrder = sealOrderLetter(
    {
      listingHashHex: params.listing.hash,
      quantity: params.quantity,
      options,
      shipTo: params.shipTo.trim(),
      rail: params.settlement === "escrow" ? "onchain" : "lightning",
      replyPublic: orderKeys.replyPublic,
      dropId: orderKeys.dropId,
    },
    params.store.storeMailboxHex,
    orderKeys.senderSecret,
  );

  const nowSecs = Math.floor(Date.now() / 1000);
  let record: LightOrderRecord = {
    version: 1,
    id: orderKeys.dropIdHex,
    checkoutId,
    dropIdHex: orderKeys.dropIdHex,
    createdAt: nowSecs,
    network: params.seed.network,
    settlement: params.settlement,
    rail,
    sellerName: params.listing.seller,
    sellerUrl: params.store.url,
    storeId: params.listing.storeId,
    storeMailboxHex: params.store.storeMailboxHex,
    storeRecordHex: params.store.storeRecordHex,
    listingRecordsHex: [listingRecordHex],
    listingHash: params.listing.hash,
    title: params.listing.title,
    quantity: params.quantity,
    estimateSats,
    fiatPriced: params.listing.price != null,
    itemsSats,
    shippingSats,
    totalSats: estimateSats,
    status: "Awaiting seller",
    paymentAddress: null,
    paymentUri: null,
    receivedSats: 0,
    confirmations: 0,
    txids: [],
    pinnedSellerHex: null,
    delivered: false,
    declinedReason: null,
    fulfillment: null,
    trackingStatus: null,
    returnPolicy: null,
    returnCase: null,
    messages: [],
    lastPulledAt: null,
    escrow: null,
  };

  const fetcher = params.fetcher ?? globalThis.fetch;
  const response = await fetcher(`${params.store.url}/mail`, {
    method: "POST",
    cache: "no-store",
    credentials: "omit",
    redirect: "error",
    referrerPolicy: "no-referrer",
    headers: { "Content-Type": "application/octet-stream" },
    body: sealedOrder as unknown as BodyInit,
  });
  if (!response.ok) {
    const errBody = (await response.json().catch(() => ({}))) as { error?: unknown };
    throw new Error(typeof errBody.error === "string" ? errBody.error : `The seller's node refused the order (HTTP ${response.status}).`);
  }
  record = { ...record, delivered: true };
  const answer = (await response.json().catch(() => ({}))) as { reply?: unknown };
  if (typeof answer.reply === "string" && /^[0-9a-f]+$/i.test(answer.reply)) {
    const opened = openSealedReply(hexToBytes(answer.reply.toLowerCase()), orderKeys.replySecret);
    const body = JSON.parse(new TextDecoder().decode(opened.body)) as Record<string, unknown>;
    record = applySellerUpdate(record, body, opened.senderHex, nowSecs);
  }
  return record;
}

/**
 * Pull the latest sealed order status/tracking/chat/return snapshot from the
 * seller's `POST /mail/pull` endpoint using the buyer's deterministic order key.
 */
export async function pullLightOrderStatus(
  order: LightOrderRecord,
  seed: BuyerSeed,
  fetcher: LightFetcher = globalThis.fetch,
): Promise<LightOrderRecord> {
  const keys = deriveOrderKeys(seed, order.checkoutId, order.storeId);
  const nonceHex = bytesToHex(randomBytes(16));
  const payload = utf8ToBytes(
    JSON.stringify({
      v: 1,
      kind: "pull",
      sale: keys.dropIdHex,
      nonce: nonceHex,
    }),
  );
  const sealed = sealMessageLetter(payload, order.storeMailboxHex, keys.senderSecret);
  const response = await fetcher(`${order.sellerUrl}/mail/pull`, {
    method: "POST",
    cache: "no-store",
    credentials: "omit",
    redirect: "error",
    referrerPolicy: "no-referrer",
    headers: { "Content-Type": "application/octet-stream" },
    body: sealed as unknown as BodyInit,
  });
  if (response.status === 403) {
    throw new Error("This order status request was not authorized by the seller.");
  }
  if (!response.ok) {
    throw new Error(`Could not pull order status (HTTP ${response.status}).`);
  }
  const answer = (await response.json()) as { reply?: unknown };
  if (typeof answer.reply !== "string" || !/^[0-9a-f]+$/i.test(answer.reply)) {
    throw new Error("The seller's status answer carried no sealed reply.");
  }
  const opened = openSealedReply(hexToBytes(answer.reply.toLowerCase()), keys.replySecret);
  const body = JSON.parse(new TextDecoder().decode(opened.body)) as Record<string, unknown>;
  if (body.kind !== "status" || body.nonce !== nonceHex) {
    throw new Error("The seller's status answer failed its freshness check.");
  }
  return applySellerUpdate(order, body, opened.senderHex);
}

/** Send a buyer-signed chat message inside a pull-mode order (`POST /mail`). */
export async function sendLightOrderMessage(
  order: LightOrderRecord,
  seed: BuyerSeed,
  text: string,
  fetcher: LightFetcher = globalThis.fetch,
): Promise<LightOrderRecord> {
  const clean = text.trim();
  if (!clean || clean.length > 2000) throw new Error("Write a message between 1 and 2000 characters.");
  const keys = deriveOrderKeys(seed, order.checkoutId, order.storeId);
  const messageId = bytesToHex(randomBytes(16));
  const payload = utf8ToBytes(
    JSON.stringify({
      v: 1,
      sale: keys.dropIdHex,
      kind: "chat",
      id: messageId,
      text: clean,
    }),
  );
  const sealed = sealMessageLetter(payload, order.storeMailboxHex, keys.senderSecret);
  const response = await fetcher(`${order.sellerUrl}/mail`, {
    method: "POST",
    cache: "no-store",
    credentials: "omit",
    redirect: "error",
    referrerPolicy: "no-referrer",
    headers: { "Content-Type": "application/octet-stream" },
    body: sealed as unknown as BodyInit,
  });
  if (!response.ok) {
    throw new Error(`The seller's node refused the message (HTTP ${response.status}).`);
  }
  const nowSecs = Math.floor(Date.now() / 1000);
  return {
    ...order,
    messages: [...order.messages, { id: messageId, from: "buyer", text: clean, at: nowSecs }],
  };
}

/** Confirm receipt on a shipped pull-mode order (`POST /mail`). */
export async function confirmLightOrderComplete(
  order: LightOrderRecord,
  seed: BuyerSeed,
  fetcher: LightFetcher = globalThis.fetch,
): Promise<LightOrderRecord> {
  if (order.status === "Completed") return order;
  if (order.status !== "Shipped") {
    throw new Error("Confirm arrival once the seller has shipped the order.");
  }
  const keys = deriveOrderKeys(seed, order.checkoutId, order.storeId);
  const payload = utf8ToBytes(JSON.stringify({ v: 1, sale: keys.dropIdHex, kind: "complete" }));
  const sealed = sealMessageLetter(payload, order.storeMailboxHex, keys.senderSecret);
  const response = await fetcher(`${order.sellerUrl}/mail`, {
    method: "POST",
    cache: "no-store",
    credentials: "omit",
    redirect: "error",
    referrerPolicy: "no-referrer",
    headers: { "Content-Type": "application/octet-stream" },
    body: sealed as unknown as BodyInit,
  });
  if (!response.ok) {
    throw new Error(`The seller's node refused the completion (HTTP ${response.status}).`);
  }
  return { ...order, status: "Completed" };
}
