// Local signed order & contract retention for buyer-only light mode.
//
// Keeps the seller's signed `StoreRecord`, signed `ListingRecord`s, verified
// payment/escrow terms, funding details, and moderator contact locally so:
// - Orders survive app restarts and offline periods.
// - If a malicious seller shuts off their node, the buyer still holds the
//   signed contract + funding details + deterministic 12-word recovery seed
//   to build and submit a dispute straight to the moderator.
// - All untrusted seller strings are checked via `isPlainText` and `displayText`.

import { sha256 } from "@noble/hashes/sha2.js";
import { schnorr } from "@noble/curves/secp256k1.js";
import { bytesToHex, concatBytes, hexToBytes, utf8ToBytes } from "@noble/hashes/utils.js";

import { isPlainText } from "../directStore.ts";
import { displayText } from "../safeText.ts";
import { verifyStoreRecord } from "../records.ts";
import {
  deriveEscrowKey,
  deriveEscrowLetterKey,
  deriveEscrowMailKey,
  derivePayoutAddress,
  taggedHash,
  type BuyerNetwork,
  type BuyerSeed,
} from "./keys.ts";
import { assertEscrowAllowed, verifySignedEscrowContract, type PaymentRailKind } from "./escrowVerify.ts";

export type LightOrderStatus =
  | "Awaiting seller"
  | "Awaiting payment"
  | "Payment seen"
  | "Paid"
  | "Shipped"
  | "Completed"
  | "Declined by the seller"
  | "Awaiting funding"
  | "Funded"
  | "Disputed";

export type LightMessage = {
  id: string;
  from: "buyer" | "seller";
  text: string;
  at: number;
};

export type LightFulfillment = {
  carrier: string;
  tracking: string;
  note: string;
  content: string;
  at?: number;
};

export type LightReturnCase = {
  state: string;
  reason?: string;
  decision?: string;
};

export type RetainedEscrowBundle = {
  escrowId: string;
  buyerKeyIndex: number;
  signedTermsHex: string;
  storeSignatureHex: string;
  storeKeyHex: string;
  listingIdHex?: string | null;
  fundingAddress: string;
  fundingOutpoint?: string | null;
  fundingTxHex?: string | null;
  refundHex?: string | null;
  buyerSnapshotHex?: string | null;
  moderatorUrl: string;
  moderatorMailKeyHex: string;
};

export type LightOrderRecord = {
  version: 1;
  id: string;
  checkoutId: string;
  dropIdHex: string;
  createdAt: number;
  network: BuyerNetwork;
  settlement: "direct" | "escrow";
  rail: PaymentRailKind;
  sellerName: string;
  sellerUrl: string;
  storeId: string;
  storeMailboxHex: string;
  /** Signed `OBST` store record hex retained locally for offline verification/disputes. */
  storeRecordHex: string;
  /** Signed `OBLS` listing record hexes retained locally. */
  listingRecordsHex: string[];
  listingHash: string;
  title: string;
  quantity: number;
  estimateSats: number;
  fiatPriced: boolean;
  itemsSats: number;
  shippingSats: number;
  totalSats: number;
  status: LightOrderStatus;
  paymentAddress: string | null;
  paymentUri: string | null;
  receivedSats: number;
  confirmations: number;
  txids: string[];
  /** Pinned seller per-sale BIP-340 sender key hex after first verified reply. */
  pinnedSellerHex: string | null;
  delivered: boolean;
  declinedReason: string | null;
  fulfillment: LightFulfillment | null;
  trackingStatus: string | null;
  returnPolicy: string | null;
  returnCase: LightReturnCase | null;
  messages: LightMessage[];
  lastPulledAt: number | null;
  escrow: RetainedEscrowBundle | null;
};

export type OfflineDisputePayload = {
  version: 1;
  network: BuyerNetwork;
  orderId: string;
  escrowId: string;
  moderatorUrl: string;
  moderatorMailKeyHex: string;
  buyerEscrowPubkeyHex: string;
  buyerPayoutAddress: string;
  buyerMailPubkeyHex: string;
  verifiedFundingAddress: string;
  storeRecordHex: string;
  signedTermsHex: string;
  storeSignatureHex: string;
  fundingOutpoint: string | null;
  refundHex: string | null;
  buyerSnapshotHex: string | null;
  memo: string;
  createdAt: number;
  /** BIP-340 signature by buyer's per-escrow letter key over the offline dispute summary. */
  claimantSignatureHex: string;
  claimantPubkeyHex: string;
};

/** Price ceiling matching `crates/node/src/remote_orders.rs::ceiling`. */
export function priceCeiling(estimateSats: number, fiatPriced: boolean): number {
  return fiatPriced ? estimateSats + Math.floor(estimateSats / 4) : estimateSats;
}

/** Check Bitcoin address prefix for `network`. */
export function isBitcoinAddressForNetwork(address: string, network: BuyerNetwork): boolean {
  const trimmed = address.trim();
  if (!trimmed || trimmed.length > 100 || /\s/.test(trimmed)) return false;
  if (network === "testnet4") {
    return /^tb1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{14,87}$/i.test(trimmed) || /^[mn2][1-9A-HJ-NP-Za-km-z]{25,34}$/.test(trimmed);
  }
  return /^bc1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{14,87}$/i.test(trimmed) || /^[13][1-9A-HJ-NP-Za-km-z]{25,34}$/.test(trimmed);
}

/** Build a BIP-21 Bitcoin payment URI for funding from any external wallet. */
export function buildBip21Uri(address: string, amountSats: number): string {
  const whole = Math.floor(amountSats / 100_000_000);
  const frac = amountSats % 100_000_000;
  const btc = frac === 0 ? String(whole) : `${whole}.${String(frac).padStart(8, "0").replace(/0+$/, "")}`;
  return `bitcoin:${address}?amount=${btc}`;
}

/** Reject hostile control or bidi override characters in untrusted seller strings. */
export function sanitizeSellerText(text: unknown, max: number, multiline: boolean): string {
  if (typeof text !== "string") return "";
  if (!isPlainText(text, multiline)) {
    throw new Error("The seller sent hostile control or direction characters.");
  }
  return displayText(text, max);
}

const STATUS_RANK: Record<LightOrderStatus, number> = {
  "Awaiting seller": 0,
  "Declined by the seller": 1,
  "Awaiting payment": 2,
  "Awaiting funding": 2,
  "Payment seen": 3,
  "Paid": 4,
  "Funded": 4,
  "Shipped": 5,
  "Disputed": 6,
  "Completed": 7,
};

function normalizeSellerStatus(raw: unknown): LightOrderStatus | null {
  if (typeof raw !== "string") return null;
  const known: readonly LightOrderStatus[] = [
    "Awaiting seller",
    "Awaiting payment",
    "Payment seen",
    "Paid",
    "Shipped",
    "Completed",
    "Declined by the seller",
    "Awaiting funding",
    "Funded",
    "Disputed",
  ];
  return known.includes(raw as LightOrderStatus) ? (raw as LightOrderStatus) : null;
}

/**
 * Apply a verified seller reply (`terms`, `declined`, or `status` pull snapshot)
 * to a retained `LightOrderRecord`.
 *
 * Enforces:
 * - Price ceiling (exact for sats; +25% max for fiat)
 * - Address network check (testnet4 vs mainnet)
 * - No backward status transitions or address/amount mutation once locked
 * - Hostile text filtering via `isPlainText` + `displayText`
 */
export function applySellerUpdate(
  order: LightOrderRecord,
  body: Record<string, unknown>,
  sellerSenderHex: string,
  nowSecs: number = Math.floor(Date.now() / 1000),
): LightOrderRecord {
  if (order.pinnedSellerHex && order.pinnedSellerHex !== sellerSenderHex) {
    throw new Error("That update was not signed by this order's seller.");
  }
  let next: LightOrderRecord = {
    ...order,
    pinnedSellerHex: sellerSenderHex,
    delivered: true,
    lastPulledAt: nowSecs,
    messages: [...order.messages],
    txids: [...order.txids],
  };

  const kind = body.kind;
  if (kind === "declined" && next.status === "Awaiting seller") {
    const reason = sanitizeSellerText(body.reason ?? "The seller declined the order.", 400, true) || "The seller declined the order.";
    return { ...next, status: "Declined by the seller", declinedReason: reason };
  }

  // Apply initial terms (either from immediate `terms` or first `status` pull).
  if ((kind === "terms" || kind === "status") && next.status === "Awaiting seller") {
    const amountSats = typeof body.amountSats === "number" && Number.isSafeInteger(body.amountSats) ? body.amountSats : 0;
    const address = typeof body.address === "string" ? body.address.trim() : "";
    const ceiling = priceCeiling(next.estimateSats, next.fiatPriced);
    if (!isBitcoinAddressForNetwork(address, next.network)) {
      return {
        ...next,
        status: "Declined by the seller",
        declinedReason: `The seller's payment address is not valid on ${next.network}. Nothing was paid.`,
      };
    }
    if (amountSats <= 0 || amountSats > ceiling) {
      return {
        ...next,
        status: "Declined by the seller",
        declinedReason: `The seller asked ${amountSats} sats; checkout showed ${next.estimateSats}. Nothing was paid.`,
      };
    }
    next = {
      ...next,
      itemsSats: typeof body.itemsSats === "number" && Number.isSafeInteger(body.itemsSats) ? body.itemsSats : next.itemsSats,
      shippingSats: typeof body.shippingSats === "number" && Number.isSafeInteger(body.shippingSats) ? body.shippingSats : next.shippingSats,
      totalSats: amountSats,
      paymentAddress: address,
      paymentUri: buildBip21Uri(address, amountSats),
      status: "Awaiting payment",
    };
  }

  if (typeof body.returnPolicy === "string" && body.returnPolicy.trim()) {
    next.returnPolicy = sanitizeSellerText(body.returnPolicy, 1000, true);
  }

  if (kind === "status") {
    if (typeof body.receivedSats === "number" && Number.isSafeInteger(body.receivedSats) && body.receivedSats >= 0) {
      next.receivedSats = body.receivedSats;
    }
    if (typeof body.confirmations === "number" && Number.isSafeInteger(body.confirmations) && body.confirmations >= 0) {
      next.confirmations = body.confirmations;
    }
    if (Array.isArray(body.txids)) {
      const cleanTxids = body.txids.filter((id): id is string => typeof id === "string" && /^[0-9a-f]{64}$/i.test(id));
      next.txids = [...new Set([...next.txids, ...cleanTxids])];
    }
    const pulledStatus = normalizeSellerStatus(body.status);
    if (pulledStatus && STATUS_RANK[pulledStatus] > STATUS_RANK[next.status] && next.paymentAddress) {
      next.status = pulledStatus;
    }
    if (body.fulfillment && typeof body.fulfillment === "object") {
      const f = body.fulfillment as Record<string, unknown>;
      next.fulfillment = {
        carrier: sanitizeSellerText(f.carrier ?? "", 60, false),
        tracking: sanitizeSellerText(f.tracking ?? "", 100, false),
        note: sanitizeSellerText(f.note ?? "", 1000, true),
        content: sanitizeSellerText(f.content ?? "", 4000, true),
        at: typeof f.at === "number" ? f.at : undefined,
      };
      if (STATUS_RANK["Shipped"] > STATUS_RANK[next.status] && next.paymentAddress) {
        next.status = "Shipped";
      }
    }
    if (body.tracking && typeof body.tracking === "object") {
      const t = body.tracking as Record<string, unknown>;
      if (typeof t.status === "string" && t.status.trim()) {
        next.trackingStatus = sanitizeSellerText(t.status, 80, false);
      }
    }
    if (body.returnCase && typeof body.returnCase === "object") {
      const rc = body.returnCase as Record<string, unknown>;
      const state = sanitizeSellerText(rc.state ?? "", 40, false);
      if (state) {
        next.returnCase = {
          state,
          reason: typeof rc.reason === "string" ? sanitizeSellerText(rc.reason, 1000, true) : undefined,
          decision: typeof rc.decision === "string" ? sanitizeSellerText(rc.decision, 1000, true) : undefined,
        };
      }
    }
    if (Array.isArray(body.messages)) {
      const seen = new Set(next.messages.map((m) => m.id));
      for (const item of body.messages) {
        if (!item || typeof item !== "object") continue;
        const msg = item as Record<string, unknown>;
        const id = typeof msg.id === "string" && /^[0-9a-f]{32}$/i.test(msg.id) ? msg.id.toLowerCase() : null;
        const from = msg.from === "buyer" || msg.from === "seller" ? msg.from : null;
        if (!id || !from || seen.has(id)) continue;
        const text = sanitizeSellerText(msg.text ?? "", 2000, true);
        if (!text) continue;
        seen.add(id);
        next.messages.push({
          id,
          from,
          text,
          at: typeof msg.at === "number" && Number.isSafeInteger(msg.at) ? msg.at : nowSecs,
        });
      }
    }
  }

  return next;
}

/**
 * Construct a self-contained dispute bundle directly from the buyer's locally
 * retained signed contract and 12-word recovery seed, without ever contacting
 * the seller.
 *
 * Refuses mainnet escrow (`ESCROW_MAINNET_REFUSAL`) and non-Bitcoin escrow,
 * re-verifies the signed store record, signed escrow terms, store signature,
 * moderator authorization, and MuSig2 Taproot funding address locally.
 */
export function buildOfflineDisputeBundle(
  order: LightOrderRecord,
  seed: BuyerSeed,
  memo: string,
  nowSecs: number = Math.floor(Date.now() / 1000),
): OfflineDisputePayload {
  assertEscrowAllowed(seed.network, order.rail);
  assertEscrowAllowed(order.network, order.rail);
  const cleanMemo = memo.trim();
  if (!cleanMemo || cleanMemo.length > 1000) {
    throw new Error("Describe the problem in 1 to 1000 characters.");
  }
  if (!isPlainText(cleanMemo, true)) {
    throw new Error("The dispute memo contains invalid control or direction characters.");
  }
  if (!order.escrow) {
    throw new Error("This order has no retained escrow contract to dispute.");
  }
  const store = verifyStoreRecord(order.storeRecordHex);
  if (!store || store.key !== order.storeId) {
    throw new Error("The retained store record failed signature verification.");
  }
  const buyerEscrow = deriveEscrowKey(seed, order.escrow.buyerKeyIndex);
  const verified = verifySignedEscrowContract({
    network: seed.network,
    rail: order.rail,
    escrowIdHex: order.escrow.escrowId,
    signedTermsHex: order.escrow.signedTermsHex,
    storeSignatureHex: order.escrow.storeSignatureHex,
    listingIdHex: order.escrow.listingIdHex,
    expectedBuyerPubkeyHex: buyerEscrow.pubkeyCompressedHex,
    claimedAddress: order.escrow.fundingAddress,
    store,
  });

  const letterKey = deriveEscrowLetterKey(seed, order.escrow.escrowId);
  const mailKey = deriveEscrowMailKey(seed);
  const payoutAddress = derivePayoutAddress(seed, order.escrow.buyerKeyIndex);

  const summaryDigest = taggedHash(
    "ob/buyer-light-dispute/v1",
    hexToBytes(order.escrow.escrowId.toLowerCase()),
    hexToBytes(verified.terms.termsDigestHex),
    sha256(utf8ToBytes(verified.derivedAddress)),
    sha256(utf8ToBytes(cleanMemo)),
    utf8ToBytes(String(nowSecs)),
  );
  const signature = schnorr.sign(summaryDigest, letterKey.secret, new Uint8Array(32));

  return {
    version: 1,
    network: seed.network,
    orderId: order.id,
    escrowId: order.escrow.escrowId,
    moderatorUrl: order.escrow.moderatorUrl,
    moderatorMailKeyHex: order.escrow.moderatorMailKeyHex,
    buyerEscrowPubkeyHex: buyerEscrow.pubkeyCompressedHex,
    buyerPayoutAddress: payoutAddress,
    buyerMailPubkeyHex: mailKey.publicHex,
    verifiedFundingAddress: verified.derivedAddress,
    storeRecordHex: order.storeRecordHex,
    signedTermsHex: order.escrow.signedTermsHex,
    storeSignatureHex: order.escrow.storeSignatureHex,
    fundingOutpoint: order.escrow.fundingOutpoint ?? null,
    refundHex: order.escrow.refundHex ?? null,
    buyerSnapshotHex: order.escrow.buyerSnapshotHex ?? null,
    memo: cleanMemo,
    createdAt: nowSecs,
    claimantSignatureHex: bytesToHex(signature),
    claimantPubkeyHex: letterKey.publicHex,
  };
}

/** Verify the claimant signature on an `OfflineDisputePayload`. */
export function verifyOfflineDisputeSignature(bundle: OfflineDisputePayload): boolean {
  try {
    const store = verifyStoreRecord(bundle.storeRecordHex);
    if (!store) return false;
    const verified = verifySignedEscrowContract({
      network: bundle.network,
      rail: "btc",
      escrowIdHex: bundle.escrowId,
      signedTermsHex: bundle.signedTermsHex,
      storeSignatureHex: bundle.storeSignatureHex,
      expectedBuyerPubkeyHex: bundle.buyerEscrowPubkeyHex,
      claimedAddress: bundle.verifiedFundingAddress,
      store,
    });
    const summaryDigest = taggedHash(
      "ob/buyer-light-dispute/v1",
      hexToBytes(bundle.escrowId.toLowerCase()),
      hexToBytes(verified.terms.termsDigestHex),
      sha256(utf8ToBytes(verified.derivedAddress)),
      sha256(utf8ToBytes(bundle.memo)),
      utf8ToBytes(String(bundle.createdAt)),
    );
    return schnorr.verify(
      hexToBytes(bundle.claimantSignatureHex),
      summaryDigest,
      hexToBytes(bundle.claimantPubkeyHex),
    );
  } catch {
    return false;
  }
}

export type BuyerLightStoreAdapter = {
  loadPhrase: () => Promise<string | null>;
  savePhrase: (phrase: string) => Promise<void>;
  loadOrders: () => Promise<LightOrderRecord[]>;
  saveOrders: (orders: LightOrderRecord[]) => Promise<void>;
  clearAll: () => Promise<void>;
};

export function createMemoryBuyerStore(): BuyerLightStoreAdapter {
  let phrase: string | null = null;
  let orders: LightOrderRecord[] = [];
  return {
    async loadPhrase() { return phrase; },
    async savePhrase(next) { phrase = next; },
    async loadOrders() { return orders.map((o) => ({ ...o })); },
    async saveOrders(next) { orders = next.map((o) => ({ ...o })); },
    async clearAll() { phrase = null; orders = []; },
  };
}
