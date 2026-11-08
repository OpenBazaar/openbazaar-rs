// One bid from the buyer, and the seller's answer. Both go to /offers.
// The node seals the bid; this page never posts it to the seller's mail.
import { paired } from "./catalog.ts";
import { toMinorUnits, type PriceCurrency } from "./seller.ts";

export const OFFER_ROUTE = "/offers";

export type OfferDraft = {
  listingHash: string;
  variant?: string;
  quantity: number;
  /** What the buyer typed, in the listing's currency. */
  amount: string;
  currency: PriceCurrency;
};

export type OfferPost = { path: string; body: { listingHash: string; variant?: string; quantity: number; amount: number; currency: string } };

/// The request the make-an-offer sheet sends. An empty amount is refused here,
/// before anything is posted.
export function offerSubmission(draft: OfferDraft): OfferPost {
  const amount = toMinorUnits(draft.amount, draft.currency);
  if (amount == null || amount <= 0) throw new Error("Enter an amount.");
  if (!Number.isInteger(draft.quantity) || draft.quantity < 1 || draft.quantity > 999) throw new Error("An offer is for 1 to 999.");
  const body: OfferPost["body"] = {
    listingHash: draft.listingHash,
    quantity: draft.quantity,
    amount,
    currency: draft.currency,
  };
  if (draft.variant) body.variant = draft.variant;
  return { path: OFFER_ROUTE, body };
}

export async function submitOffer(draft: OfferDraft): Promise<void> {
  const { path, body } = offerSubmission(draft);
  await paired(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
}

export type WaitingBid = {
  id: string;
  listingId: string;
  variantId: string;
  quantity: number;
  amount: number;
  currency: string;
};

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("The node sent an offer the page could not read.");
  return value as Record<string, unknown>;
}

/// Bids waiting on this store. A conversation key or reply route is not kept.
export function parseWaitingBids(value: unknown): WaitingBid[] {
  const items = record(value).offers;
  if (!Array.isArray(items)) return [];
  return items.map(item => {
    const row = record(item);
    const id = row.id;
    const listingId = row.listingId;
    const variantId = row.variantId;
    const quantity = row.quantity;
    const amount = row.amount;
    const currency = row.currency;
    if (typeof id !== "string" || typeof listingId !== "string" || typeof variantId !== "string" || typeof currency !== "string") {
      throw new Error("The node sent an offer the page could not read.");
    }
    if (!Number.isInteger(quantity) || !Number.isInteger(amount)) throw new Error("The node sent an offer the page could not read.");
    return { id, listingId, variantId, quantity: quantity as number, amount: amount as number, currency };
  });
}

export async function fetchWaitingBids(): Promise<WaitingBid[]> {
  return parseWaitingBids(await paired(OFFER_ROUTE));
}

export type OfferAnswer = { decline?: string; amount?: number; expiresAt?: number };

/// The seller's answer, on the route that already signs or declines a bid.
export function answerSubmission(id: string, answer: OfferAnswer): { path: string; body: OfferAnswer } {
  if (!/^[0-9a-f]{32}$/.test(id)) throw new Error("That offer is not waiting for an answer.");
  const path = `${OFFER_ROUTE}/${id}/answer`;
  if (answer.decline != null) {
    const decline = answer.decline.trim();
    if (!decline) throw new Error("Say why the bid is declined.");
    return { path, body: { decline } };
  }
  if (answer.expiresAt == null) throw new Error("An offer needs an expiry.");
  const body: OfferAnswer = { expiresAt: answer.expiresAt };
  if (answer.amount != null) body.amount = answer.amount;
  return { path, body };
}

export async function answerOffer(id: string, answer: OfferAnswer): Promise<void> {
  const { path, body } = answerSubmission(id, answer);
  await paired(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
}
