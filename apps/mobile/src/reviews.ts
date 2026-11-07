import { paired } from "./catalog.ts";

export type StoreReview = {
  id: string;
  listingId: string;
  listingTitle: string | null;
  rating: number;
  text: string;
  writtenOn: number;
  paidMonth: number;
  reply: { text: string; writtenOn: number } | null;
};

/** A store's verified-purchase reviews as this node holds them, and their stars. */
export type StoreReviews = { count: number; average: number | null; stars: number[]; reviews: StoreReview[] };

const text = (v: unknown) => (typeof v === "string" ? v : "");
const number = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);

export function parseStoreReviews(value: unknown): StoreReviews {
  const body = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const reviews = (Array.isArray(body.reviews) ? body.reviews : []).flatMap((item): StoreReview[] => {
    if (!item || typeof item !== "object") return [];
    const r = item as Record<string, unknown>;
    const rating = number(r.rating);
    if (rating < 1 || rating > 5 || !text(r.id)) return [];
    const reply = r.reply && typeof r.reply === "object" ? (r.reply as Record<string, unknown>) : null;
    return [{
      id: text(r.id), listingId: text(r.listingId), listingTitle: typeof r.listingTitle === "string" ? r.listingTitle : null, rating,
      text: text(r.text), writtenOn: number(r.writtenOn), paidMonth: number(r.paidMonth),
      reply: reply && text(reply.text) ? { text: text(reply.text), writtenOn: number(reply.writtenOn) } : null,
    }];
  });
  const stars = Array.isArray(body.stars) && body.stars.length === 5 ? body.stars.map(number) : [0, 0, 0, 0, 0];
  return { count: reviews.length, average: typeof body.average === "number" ? body.average : null, stars, reviews };
}

/** The same reviews limited to one listing, with stars recounted. */
export function forListing(all: StoreReviews, listingId: string | null): StoreReviews {
  const reviews = all.reviews.filter(r => r.listingId === listingId);
  const stars = [1, 2, 3, 4, 5].map(n => reviews.filter(r => r.rating === n).length);
  const average = reviews.length ? Math.floor((reviews.reduce((n, r) => n + r.rating, 0) * 10) / reviews.length) / 10 : null;
  return { count: reviews.length, average, stars, reviews };
}

export async function fetchStoreReviews(storeKey: string): Promise<StoreReviews> {
  return parseStoreReviews(await paired(`/stores/${storeKey}/reviews`));
}

const post = (body: unknown): RequestInit => ({ method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

export async function writeReview(orderId: string, rating: number, body: string): Promise<void> {
  await paired(`/orders/${orderId}/review`, post({ rating, text: body }));
}

export async function replyToReview(id: string, body: string): Promise<StoreReviews> {
  return parseStoreReviews(await paired(`/reviews/${id}/reply`, post({ text: body })));
}

/** "Nov 2026" from YYYYMM, or "Nov 12, 2026" from YYYYMMDD. */
export function reviewDate(value: number): string {
  const day = value > 999_999 ? value % 100 : 0;
  const month = value > 999_999 ? Math.floor(value / 100) % 100 : value % 100;
  const year = value > 999_999 ? Math.floor(value / 10_000) : Math.floor(value / 100);
  const date = new Date(Date.UTC(year, month - 1, day || 1));
  return date.toLocaleDateString("en-US", { month: "short", ...(day ? { day: "numeric" } : {}), year: "numeric", timeZone: "UTC" });
}
