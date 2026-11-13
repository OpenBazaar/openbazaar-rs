import type { ShipmentTracking } from "../shipment";

function when(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

function place(event: ShipmentTracking["events"][number]): string {
  return [event.city, event.state, event.country].filter(Boolean).join(", ");
}

/** Carrier progress, received from the seller's node rather than a live carrier query. */
export function TrackingTimeline({ tracking, carrier, trackingNumber }: { tracking: ShipmentTracking | null; carrier: string; trackingNumber: string }) {
  if (!carrier && !trackingNumber && !tracking) return null;
  const events = tracking ? [...tracking.events].reverse() : [];
  return <section className="ob-tracking" aria-label="Shipment tracking">
    <div className="ob-tracking-head">
      <div>
        <h3>Shipment tracking</h3>
        <p>{[carrier, trackingNumber].filter(Boolean).join(" · ")}</p>
      </div>
      <strong>{tracking?.label ?? "Tracking entered by the seller"}</strong>
    </div>
    {tracking?.deliveredAt && <p className="ob-tracking-delivered">Delivered {when(tracking.deliveredAt)}</p>}
    {events.length > 0 && <ol className="ob-tracking-events">
      {events.map((event, index) => <li key={`${event.at}-${event.status}-${index}`}>
        <span className="ob-tracking-dot" aria-hidden="true"/>
        <div><strong>{event.label}</strong><span>{when(event.at)}{place(event) ? ` · ${place(event)}` : ""}</span></div>
      </li>)}
    </ol>}
    <p className="ob-tracking-privacy">Updates arrive through the seller’s node. Your node does not contact the carrier or tracking service.</p>
  </section>;
}
