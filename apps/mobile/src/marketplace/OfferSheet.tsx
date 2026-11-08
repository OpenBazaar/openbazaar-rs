import { useState, type FormEvent } from "react";
import type { ListingHeader } from "../catalog";
import { offerSubmission, submitOffer } from "../offers";
import { formatPrice, type PriceCurrency } from "../seller";
import type { BuyChoice } from "./ListingBuy";

/// One sealed bid for the variant and quantity already chosen on this listing.
export function OfferSheet({ listing, choice }: { listing: ListingHeader; choice: BuyChoice }) {
  const [open, setOpen] = useState(false);
  const [amount, setAmount] = useState("");
  const [error, setError] = useState("");
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);
  const price = listing.price;
  if (!listing.commerce?.acceptsOffers || !price || !listing.listingId) return null;
  const currency = price.currency as PriceCurrency;

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError("");
    let post;
    try {
      post = offerSubmission({ listingHash: listing.hash, variant: choice.variant, quantity: choice.quantity, amount, currency });
    } catch (err) {
      setError(err instanceof Error ? err.message : "Enter an amount.");
      return;
    }
    setBusy(true);
    try {
      await submitOffer({ listingHash: post.body.listingHash, variant: post.body.variant, quantity: post.body.quantity, amount, currency });
      setSent(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : "The offer could not be sent.");
    } finally {
      setBusy(false);
    }
  }

  if (sent) return <p className="ob-shipping-note" role="status">Offer sent. The seller can accept, counter, or decline. You are not named.</p>;
  if (!open) return <button type="button" className="ob-secondary" onClick={() => setOpen(true)}>Make an offer</button>;
  return <form className="ob-field-grid" onSubmit={event => void submit(event)} aria-label={`Make an offer on ${listing.title}`}>
    <label className="ob-field ob-field-wide"><span>Your price ({currency})</span>
      <input inputMode="decimal" autoComplete="off" value={amount} placeholder={formatPrice(price.amount, currency)} onChange={event => setAmount(event.target.value)}/>
    </label>
    <p className="ob-shipping-note">One offer for {choice.quantity}, in this listing's currency. The seller answers in private.</p>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions">
      <button type="submit" className="ob-primary" disabled={busy}>{busy ? "Sending…" : "Send offer"}</button>
      <button type="button" className="ob-secondary" disabled={busy} onClick={() => { setOpen(false); setError(""); }}>Cancel</button>
    </div>
  </form>;
}
