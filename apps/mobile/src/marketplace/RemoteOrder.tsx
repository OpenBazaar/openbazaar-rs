import { useState } from "react";
import type { BuyerOrder } from "../catalog";
import { completeOrder } from "../seller";
import { OrderReviewForm } from "./Reviews";
import { TrackingTimeline } from "./TrackingTimeline";

/** Delivery and seller replies for an order to a store on another node. */
export function RemoteOrderStatus({ order, onChange }: { order: BuyerOrder; onChange: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const remote = order.remote;
  if (!remote) return null;
  async function complete() {
    setBusy(true);
    setError("");
    try {
      await completeOrder(order.id);
      onChange();
    } catch (e) {
      setError(e instanceof Error ? e.message : "The order could not be completed.");
    } finally {
      setBusy(false);
    }
  }
  const sent = remote.fulfillment;
  return <div className="ob-remote-order" aria-live="polite">
    {order.status === "Awaiting seller" && <p className="ob-page-intro">{remote.delivered ? `${order.seller} has your order. Waiting for the seller's node to send payment terms.` : remote.error ?? "Sending your order to the seller's node…"}</p>}
    {order.status === "Declined by the seller" && <p className="ob-form-error" role="alert">{remote.reason ?? "The seller declined this order."} Nothing was paid.</p>}
    {remote.sellerSawPayment && order.status !== "Completed" && <p className="ob-page-intro">The seller's node confirmed your payment.</p>}
    {remote.oversold && <p className="ob-form-error" role="alert">The seller sold out before your payment arrived. Message {order.seller} to arrange your refund.</p>}
    {sent && <div className="ob-remote-fulfillment">
      <strong>{sent.content ? "Delivered" : "Shipped"}</strong>
      {!sent.content && (sent.carrier || sent.tracking) && <TrackingTimeline tracking={remote.tracking} carrier={sent.carrier} trackingNumber={sent.tracking}/>}
      {sent.note && <p>{sent.note}</p>}
      {sent.content && <p className="ob-sale-content">{sent.content}</p>}
    </div>}
    {order.status === "Shipped" && <button className="ob-primary" disabled={busy} onClick={() => void complete()}>{busy ? "Confirming…" : "Confirm it arrived"}</button>}
    {order.status === "Completed" && <p className="ob-page-intro">You confirmed this order arrived. It is complete.</p>}
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <OrderReviewForm order={order} onChange={onChange}/>
  </div>;
}
