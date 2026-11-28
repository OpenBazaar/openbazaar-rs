// Bookmarkable / copyable `#receipt=...` 1-tap restore links for Tor Browser.
//
// Security invariant: the 12-word receipt phrase is ONLY ever placed inside the
// URL `#fragment` — never in query `?` parameters — so browsers never send it
// over HTTP to the moderator or vendor server.

import { isValidBuyerPhrase, normalizeBuyerPhrase } from "../buyerLight/keys.ts";

export type ParsedReceiptHash = {
  readonly phrase: string;
  readonly stores: readonly string[];
  readonly cids: readonly string[];
};

const MAX_RECEIPT_CIDS = 20;
const SAFE_CHECKOUT_ID_RE = /^[a-zA-Z0-9_-]{1,64}$/;
const DETERMINISTIC_TOR_CID_RE = /^tor-\d+$/;

export function normalizeReceiptCheckoutId(raw: string): string | null {
  const clean = raw.trim();
  if (!clean || !SAFE_CHECKOUT_ID_RE.test(clean)) return null;
  return clean;
}

export const RECEIPT_LINK_HELPER_TEXT =
  "Save in Notes or Bookmarks — opens your orders in 1 tap on a fresh Tor session. The link fragment stays on your device and is never sent to the server.";

function normalizeOrigin(raw: string): string | null {
  try {
    const parsed = new URL(raw.trim());
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    if (parsed.username || parsed.password) return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

/**
 * Validate and normalize a store URL carried in a `#receipt=...` link.
 * Only `http://*.onion` URLs or URLs matching `currentOrigin` are accepted.
 */
export function normalizeReceiptStoreUrl(raw: string, currentOrigin?: string): string | null {
  try {
    const parsed = new URL(raw.trim());
    if (parsed.username || parsed.password) return null;
    const host = parsed.hostname.toLowerCase();
    const isOnion = parsed.protocol === "http:" && host.endsWith(".onion") && host.length > ".onion".length;
    if (isOnion) {
      return parsed.origin;
    }
    if (currentOrigin) {
      const normalizedCurrent = normalizeOrigin(currentOrigin);
      if (normalizedCurrent && parsed.origin === normalizedCurrent) {
        return parsed.origin;
      }
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Build a 1-tap restore URL `${origin}/#receipt=...` with the normalized
 * 12-word phrase, optional store `.onion` URL(s), and optional legacy
 * non-deterministic checkout IDs strictly in the hash.
 */
export function buildReceiptRestoreUrl(
  origin: string,
  phrase: string,
  vendorUrls?: readonly string[],
  extraCheckoutIds?: readonly string[],
): string {
  const baseOrigin = normalizeOrigin(origin);
  if (!baseOrigin) {
    throw new Error("Invalid origin for receipt restore link.");
  }
  const cleanPhrase = normalizeBuyerPhrase(phrase);
  if (!isValidBuyerPhrase(cleanPhrase)) {
    throw new Error("Invalid 12-word BIP-39 receipt phrase.");
  }

  const params = new URLSearchParams();
  params.set("receipt", cleanPhrase);

  if (vendorUrls && vendorUrls.length > 0) {
    const seen = new Set<string>();
    for (const candidate of vendorUrls) {
      const normalized = normalizeReceiptStoreUrl(candidate, baseOrigin);
      if (!normalized || seen.has(normalized)) continue;
      seen.add(normalized);
      params.append("store", normalized);
    }
  }

  if (extraCheckoutIds && extraCheckoutIds.length > 0) {
    const seenCids = new Set<string>();
    for (const candidate of extraCheckoutIds) {
      if (seenCids.size >= MAX_RECEIPT_CIDS) break;
      const normalized = normalizeReceiptCheckoutId(candidate);
      if (!normalized || DETERMINISTIC_TOR_CID_RE.test(normalized) || seenCids.has(normalized)) continue;
      seenCids.add(normalized);
      params.append("cid", normalized);
    }
  }

  return `${baseOrigin}/#${params.toString()}`;
}

/**
 * Parse a URL `#fragment` carrying `#receipt=...` (and optional `&store=...` / `&cid=...`).
 * Returns `null` for missing, non-receipt, or invalid BIP-39 hashes.
 */
export function parseReceiptHash(hash: string, currentOrigin?: string): ParsedReceiptHash | null {
  if (!hash || hash.startsWith("?")) return null;
  const trimmed = hash.trim();
  const rawBody = trimmed.startsWith("#") ? trimmed.slice(1) : trimmed;
  if (!rawBody || rawBody.startsWith("/")) return null;

  const params = new URLSearchParams(rawBody);
  const rawReceipt = params.get("receipt");
  if (!rawReceipt) return null;

  const cleanPhrase = normalizeBuyerPhrase(rawReceipt);
  if (!isValidBuyerPhrase(cleanPhrase)) return null;

  const stores: string[] = [];
  const seen = new Set<string>();
  for (const rawStore of params.getAll("store")) {
    const normalized = normalizeReceiptStoreUrl(rawStore, currentOrigin);
    if (!normalized || seen.has(normalized)) continue;
    seen.add(normalized);
    stores.push(normalized);
  }

  const cids: string[] = [];
  const seenCids = new Set<string>();
  for (const rawCid of params.getAll("cid")) {
    if (seenCids.size >= MAX_RECEIPT_CIDS) break;
    const normalized = normalizeReceiptCheckoutId(rawCid);
    if (!normalized || DETERMINISTIC_TOR_CID_RE.test(normalized) || seenCids.has(normalized)) continue;
    seenCids.add(normalized);
    cids.push(normalized);
  }

  return { phrase: cleanPhrase, stores, cids };
}
