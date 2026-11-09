import { useState } from "react";
import { answerOffer, answerSubmission, fetchWaitingBids, type WaitingBid } from "../offers";
import { formatPrice, fromMinorUnits, toMinorUnits, type PriceCurrency } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { useLockedAction } from "./SellerKit";

const DAY = 86_400;

/// Bids waiting on this store. Accept, counter, or decline with the answer route.
export function OfferInbox() {
  const bids = useNodeResource(fetchWaitingBids, 15000, true, list => list.length === 0);
  if (bids.error) return <p className="ob-form-error" role="alert">{bids.error}</p>;
  if (!bids.data?.length) return null;
  return <div className="ob-orders">
    <h2>Offers</h2>
    {bids.data.map(bid => <BidRow key={bid.id} bid={bid} onDone={() => void bids.refresh()}/>)}
  </div>;
}

function BidRow({ bid, onDone }: { bid: WaitingBid; onDone: () => void }) {
  const [days, setDays] = useState("");
  const [counter, setCounter] = useState("");
  const [reason, setReason] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const { run, dialog } = useLockedAction(setError);
  const currency = bid.currency as PriceCurrency;
  const variant = bid.variantId === "0".repeat(16) ? "" : bid.variantId;

  function expiry(): number {
    const count = Number(days);
    if (!Number.isInteger(count) || count < 1 || count > 365) throw new Error("Say how many days the offer lasts.");
    return Math.floor(Date.now() / 1000) + count * DAY;
  }

  function send(answer: Parameters<typeof answerSubmission>[1]) {
    setError("");
    setBusy(true);
    void run(async () => {
      try {
        answerSubmission(bid.id, answer);
        await answerOffer(bid.id, answer);
        onDone();
      } finally {
        setBusy(false);
      }
    });
  }

  return <article className="ob-order-row ob-offer-response">
    <div className="ob-order-expanded">
      <p><strong>{formatPrice(bid.amount, bid.currency)}</strong> for {bid.quantity}{variant ? ` · option ${variant}` : ""}</p>
      <form className="ob-field-grid" onSubmit={event => {
        event.preventDefault();
        try {
          const amount = counter.trim() ? toMinorUnits(counter, currency) : null;
          if (counter.trim() && (amount == null || amount <= 0)) throw new Error("Enter an amount.");
          send({ expiresAt: expiry(), ...(amount ? { amount } : {}) });
        } catch (err) {
          setError(err instanceof Error ? err.message : "The offer could not be sent.");
        }
      }}>
        <label className="ob-field"><span>Good for (days)</span><input inputMode="numeric" value={days} placeholder="7" onChange={event => setDays(event.target.value)}/></label>
        <label className="ob-field"><span>Counter ({currency}, optional)</span><input inputMode="decimal" value={counter} placeholder={fromMinorUnits(bid.amount, currency)} onChange={event => setCounter(event.target.value)}/></label>
        <div className="ob-form-actions"><button className="ob-primary" disabled={busy}>{counter.trim() ? "Send counter" : "Accept"}</button></div>
      </form>
      <form className="ob-field-grid" onSubmit={event => {
        event.preventDefault();
        try {
          send(answerSubmission(bid.id, { decline: reason }).body);
        } catch (err) {
          setError(err instanceof Error ? err.message : "Say why the bid is declined.");
        }
      }}>
        <label className="ob-field ob-field-wide"><span>Decline</span><input value={reason} maxLength={1000} onChange={event => setReason(event.target.value)}/></label>
        <div className="ob-form-actions"><button className="ob-secondary" disabled={busy}>Decline</button></div>
      </form>
      {error && <p className="ob-form-error" role="alert">{error}</p>}
      {dialog}
    </div>
  </article>;
}
