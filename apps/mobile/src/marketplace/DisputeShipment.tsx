import { useState } from "react";
import { lookUpDisputeShipment, type DisputeTracking } from "../moderator";
import { CARRIER_OPTIONS } from "../shipment";

// The carriers the node can look up live; "Other" is manual-entry only.
const LOOKUP_CARRIERS = CARRIER_OPTIONS.filter(option => option.value !== "Other");

const failed = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

function when(at: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

const place = (city: string, state: string, country: string) => [city, state, country].filter(Boolean).join(", ");

const VERDICTS = {
  match: "Delivered where expected",
  mismatch: "Delivered somewhere else",
  unverifiable: "Locality not verifiable",
} as const;

function carrierOption(slug: string): string {
  return LOOKUP_CARRIERS.find(option => option.value.toLowerCase() === slug)?.value ?? "USPS";
}

/**
 * This node's own carrier lookup for a disputed shipment. The operator copies
 * the carrier, tracking number, and (from the buyer's letter) the expected
 * ship-to locality out of the memos; the node asks the tracking provider
 * itself over Tor. The answer is evidence the moderator observed first-hand,
 * and it is advisory: no checkpoint shows what the parcel held.
 */
export function DisputeShipment({ id, tracking, onLooked }: { id: string; tracking: DisputeTracking | null; onLooked: () => Promise<void> | void }) {
  const [carrier, setCarrier] = useState(() => (tracking ? carrierOption(tracking.carrier) : "USPS"));
  const [number, setNumber] = useState(tracking?.trackingNumber ?? "");
  const [city, setCity] = useState(tracking?.shipTo?.city ?? "");
  const [state, setState] = useState(tracking?.shipTo?.state ?? "");
  const [country, setCountry] = useState(tracking?.shipTo?.country ?? "");
  const [looking, setLooking] = useState(false);
  const [error, setError] = useState("");
  const check = tracking?.locationCheck ?? null;
  const events = tracking ? [...tracking.events].reverse() : [];

  async function lookUp() {
    if (looking) return;
    if (!number.trim()) { setError("Enter the tracking number from the dispute statements."); return; }
    if (!city.trim() && (state.trim() || country.trim())) { setError("A ship-to check needs at least the expected city."); return; }
    setLooking(true);
    setError("");
    try {
      await lookUpDisputeShipment(id, {
        carrier,
        trackingNumber: number,
        ...(city.trim() ? { shipTo: { city: city.trim(), state: state.trim(), country: country.trim() } } : {}),
      });
      await onLooked();
    } catch (err) {
      setError(failed(err, "The shipment could not be looked up."));
    } finally {
      setLooking(false);
    }
  }

  return <section className="ob-case-shipment" aria-label="Shipment check">
    <header><h3>Shipment check</h3><span>This node asks the carrier’s tracking service itself, not the parties.</span></header>
    <div className="ob-case-shipment-form">
      <label>Carrier<select value={carrier} onChange={event => setCarrier(event.target.value)}>{LOOKUP_CARRIERS.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
      <label>Tracking number<input value={number} onChange={event => setNumber(event.target.value)} placeholder="From the statements above" autoComplete="off"/></label>
      <label>Expected city<input value={city} onChange={event => setCity(event.target.value)} placeholder="Optional, from the buyer’s letter" autoComplete="off"/></label>
      <label>Expected state<input value={state} onChange={event => setState(event.target.value)} placeholder="Optional" autoComplete="off"/></label>
      <label>Expected country<input value={country} onChange={event => setCountry(event.target.value)} placeholder="Optional" autoComplete="off"/></label>
      <button className="ob-secondary" disabled={looking} onClick={() => void lookUp()}>{looking ? "Asking the carrier…" : tracking ? "Look up again" : "Look shipment up"}</button>
    </div>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    {tracking && <div className="ob-case-shipment-result">
      <p><strong>{tracking.label}</strong>{tracking.deliveredAt ? ` · ${when(tracking.deliveredAt)}` : ""} · {tracking.carrier.toUpperCase()} {tracking.trackingNumber}</p>
      {check && <p className={`ob-shipment-verdict is-${check.verdict}`}>
        <strong>{VERDICTS[check.verdict]}.</strong> {check.reason}
        {check.verdict !== "unverifiable" && tracking.shipTo && <> Expected {place(tracking.shipTo.city, tracking.shipTo.state, tracking.shipTo.country)}; the carrier names {place(check.deliveredCity, check.deliveredState, check.deliveredCountry) || "no place"}.</>}
      </p>}
      {events.length > 0 && <ol className="ob-tracking-events">
        {events.map((event, index) => <li key={`${event.at}-${event.status}-${index}`}>
          <span className="ob-tracking-dot" aria-hidden="true"/>
          <div><strong>{event.label}</strong><span>{when(event.at)}{place(event.city, event.state, event.country) ? ` · ${place(event.city, event.state, event.country)}` : ""}</span></div>
        </li>)}
      </ol>}
    </div>}
    <p className="ob-case-shipment-note">The lookup leaves this node through Tor and sends only the carrier and tracking number. A carrier scan is advisory evidence: it cannot show what the parcel held, and carriers misreport places, so it decides nothing by itself.</p>
  </section>;
}
