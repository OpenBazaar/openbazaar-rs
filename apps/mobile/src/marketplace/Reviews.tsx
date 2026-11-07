import { useEffect, useState } from "react";
import type { BuyerOrder, ListingHeader } from "../catalog";
import { fetchStoreReviews, forListing, replyToReview, reviewDate, writeReview, type StoreReview, type StoreReviews } from "../reviews";
import { useListingStyles } from "./VariantPicker";

export function Stars({ rating, size = "small" }: { rating: number; size?: "small" | "large" }) {
  const halves = Math.round(rating * 2);
  return <span className={`ob-stars is-${size}`} role="img" aria-label={`${rating} out of 5 stars`}>
    {[1, 2, 3, 4, 5].map(n => <span key={n} className={n * 2 <= halves ? "is-on" : n * 2 - 1 === halves ? "is-half" : ""} aria-hidden="true">★</span>)}
  </span>;
}

function useStoreReviews(storeKey: string) {
  const [reviews, setReviews] = useState<StoreReviews | null>(null);
  useEffect(() => {
    let active = true;
    fetchStoreReviews(storeKey).then(r => { if (active) setReviews(r); }).catch(() => { if (active) setReviews(null); });
    return () => { active = false; };
  }, [storeKey]);
  return [reviews, setReviews] as const;
}

function Summary({ reviews, label }: { reviews: StoreReviews; label: string }) {
  if (!reviews.count || reviews.average === null) return <p className="ob-review-none">No verified reviews {label} yet.</p>;
  return <div className="ob-review-summary">
    <Stars rating={reviews.average} size="large"/>
    <strong>{reviews.average.toFixed(1)}</strong>
    <span>{reviews.count} verified {reviews.count === 1 ? "review" : "reviews"} {label}</span>
  </div>;
}

function ReplyForm({ review, onSaved }: { review: StoreReview; onSaved: (all: StoreReviews) => void }) {
  const [open, setOpen] = useState(false);
  const [text, setText] = useState(review.reply?.text ?? "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  if (!open) return <button type="button" className="ob-text-link" onClick={() => setOpen(true)}>{review.reply ? "Change your reply" : "Reply"}</button>;
  async function save() {
    setBusy(true);
    setError("");
    try { onSaved(await replyToReview(review.id, text)); setOpen(false); } catch (e) { setError(e instanceof Error ? e.message : "The reply could not be signed."); } finally { setBusy(false); }
  }
  return <div className="ob-review-reply-form">
    <label className="ob-field"><span>Your public reply</span><textarea value={text} maxLength={1000} onChange={e => setText(e.target.value)}/></label>
    <p className="ob-field-hint">Signed with your store key and shown under the review. You can change your reply; you cannot change or remove the review.</p>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions"><button type="button" className="ob-secondary" onClick={() => setOpen(false)}>Cancel</button><button type="button" className="ob-primary" disabled={busy || !text.trim()} onClick={() => void save()}>{busy ? "Signing…" : "Sign reply"}</button></div>
  </div>;
}

function ReviewList({ reviews, own, onChange, showListing }: { reviews: StoreReview[]; own: boolean; onChange: (all: StoreReviews) => void; showListing?: boolean }) {
  return <ul className="ob-review-list">{reviews.map(r => <li key={r.id} className="ob-review">
    <div className="ob-review-head"><Stars rating={r.rating}/><span className="ob-review-badge">Verified purchase</span><span className="ob-meta">{reviewDate(r.writtenOn)} · bought {reviewDate(r.paidMonth)}</span></div>
    {showListing && r.listingTitle && <p className="ob-review-listing">{r.listingTitle}</p>}
    {r.text && <p className="ob-review-text">{r.text}</p>}
    {r.reply && <div className="ob-review-reply"><strong>Seller's reply</strong><p>{r.reply.text}</p></div>}
    {own && <ReplyForm review={r} onSaved={onChange}/>}
  </li>)}</ul>;
}

/// The listing's reviews and the store's stars, on the listing page.
export function ListingReviews({ listing, own }: { listing: ListingHeader; own: boolean }) {
  useListingStyles();
  const [all, setAll] = useStoreReviews(listing.storeId);
  if (!all) return null;
  const mine = forListing(all, listing.listingId);
  return <section className="ob-reviews" aria-labelledby="ob-reviews-title">
    <h2 id="ob-reviews-title">Reviews</h2>
    <Summary reviews={mine} label="for this item"/>
    {all.count > mine.count && all.average !== null && <p className="ob-meta">{listing.seller}: {all.average.toFixed(1)} from {all.count} verified reviews across the store</p>}
    <ReviewList reviews={mine.reviews} own={own} onChange={setAll}/>
    <p className="ob-review-note">Only a buyer with the seller's signed receipt for a paid order can review it, once per order. Reviews carry no name or account, and nodes keep copies, so a seller cannot remove one.</p>
  </section>;
}

/// Every review of a store, for its storefront.
export function StoreReviewsPanel({ storeKey, own }: { storeKey: string; own: boolean }) {
  useListingStyles();
  const [all, setAll] = useStoreReviews(storeKey);
  if (!all) return null;
  return <div className="ob-reviews is-store">
    <h3>Reviews</h3>
    <Summary reviews={all} label=""/>
    <ReviewList reviews={all.reviews} own={own} onChange={setAll} showListing/>
  </div>;
}

/// Write or change the review of a paid order.
export function OrderReviewForm({ order, onChange }: { order: BuyerOrder; onChange: () => void }) {
  useListingStyles();
  const kept = order.remote?.review ?? null;
  const [rating, setRating] = useState(kept?.rating ?? 0);
  const [text, setText] = useState(kept?.text ?? "");
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const paid = ["Paid", "Shipped", "Completed"].includes(order.status);
  if (!order.remote || !paid || order.remote.receipts === 0) return null;
  if (kept && !editing) {
    const when = kept.published && kept.writtenOn ? `Published ${reviewDate(kept.writtenOn)}` : `Publishes after ${new Date(kept.sendAfter * 1000).toLocaleDateString("en-US", { month: "short", day: "numeric" })}`;
    return <div className="ob-order-review">
      <div className="ob-review-head"><Stars rating={kept.rating}/><span className="ob-meta">{when}</span></div>
      {kept.text && <p className="ob-review-text">{kept.text}</p>}
      <button type="button" className="ob-text-link" onClick={() => setEditing(true)}>Change your review</button>
    </div>;
  }
  async function save() {
    setBusy(true);
    setError("");
    try { await writeReview(order.id, rating, text); setEditing(false); onChange(); } catch (e) { setError(e instanceof Error ? e.message : "The review could not be saved."); } finally { setBusy(false); }
  }
  return <div className="ob-order-review">
    <strong>Review this order</strong>
    <div className="ob-star-picker" role="radiogroup" aria-label="Stars">
      {[1, 2, 3, 4, 5].map(n => <button key={n} type="button" role="radio" aria-checked={rating === n} aria-label={`${n} star${n === 1 ? "" : "s"}`} className={n <= rating ? "is-on" : ""} onClick={() => setRating(n)}>★</button>)}
    </div>
    <label className="ob-field"><span>What should other buyers know? (optional)</span><textarea value={text} maxLength={1000} onChange={e => setText(e.target.value)}/></label>
    <p className="ob-field-hint">Signed with a key made for this order only, so it does not link to your other orders or reviews. Your node publishes it within three days, dated the day it goes out, and other nodes keep copies. Leave out names, addresses and contact details.</p>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions">{kept && <button type="button" className="ob-secondary" onClick={() => setEditing(false)}>Cancel</button>}<button type="button" className="ob-primary" disabled={busy || rating === 0} onClick={() => void save()}>{busy ? "Saving…" : "Save review"}</button></div>
  </div>;
}
