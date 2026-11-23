import { useState, type FormEvent } from "react";
import { CARRIER_OPTIONS } from "../shipment";
import { parseReturnPolicy, returnStateLabel, type ReturnCase as ReturnCaseRecord } from "../returnCase";

const open = (status: string) => status === "Shipped" || status === "Completed";

/** The return on one order: the policy, the case, and the next step for this side. */
export function ReturnCase({
  viewer,
  status,
  policy,
  returnCase,
  allowShipment,
  onRequest,
  onDecide,
  onShip,
}: {
  viewer: "buyer" | "seller";
  status: string;
  policy: string;
  returnCase: ReturnCaseRecord | null;
  /** A physical order can name a carrier and tracking number for the trip back. */
  allowShipment: boolean;
  onRequest?: (text: string) => Promise<void>;
  onDecide?: (decision: "accept" | "decline", text: string) => Promise<void>;
  onShip?: (shipment: { carrier: string; tracking: string; note: string }) => Promise<void>;
}) {
  const stated = parseReturnPolicy(policy);
  if (!stated && !returnCase) return null;
  const canRequest = viewer === "buyer" && !returnCase && open(status) && stated.length > 0 && !!onRequest;
  const canDecide = viewer === "seller" && returnCase?.state === "requested" && !!onDecide;
  const canShip = viewer === "buyer" && returnCase?.state === "accepted" && allowShipment && !!onShip;
  return <section className="ob-return-case" aria-label="Return">
    <h3>{returnCase ? returnStateLabel(returnCase.state) : "Returns"}</h3>
    {stated && <p className="ob-return-policy">{stated}</p>}
    {returnCase?.reason && <p>{returnCase.reason}</p>}
    {returnCase?.decision && <p>{returnCase.decision}</p>}
    {returnCase?.state === "shipped" && <ReturnShipment shipment={returnCase.shipment}/>}
    {returnCase?.state === "declined" && <p className="ob-return-ended">This return is closed.</p>}
    {canRequest && onRequest && <RequestForm onRequest={onRequest}/>}
    {canDecide && onDecide && <DecideForm onDecide={onDecide}/>}
    {canShip && onShip && <ShipForm onShip={onShip}/>}
  </section>;
}

function ReturnShipment({ shipment }: { shipment: ReturnCaseRecord["shipment"] }) {
  if (!shipment) return null;
  const reference = [shipment.carrier, shipment.tracking].filter(Boolean).join(" · ");
  return <div className="ob-return-shipment">
    {reference && <p>{reference}</p>}
    {shipment.note && <p>{shipment.note}</p>}
  </div>;
}

function RequestForm({ onRequest }: { onRequest: (text: string) => Promise<void> }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await onRequest(text);
      setText("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "The return could not be opened.");
    } finally {
      setBusy(false);
    }
  }
  return <form className="ob-return-form" onSubmit={event => void submit(event)}>
    <label className="ob-field ob-field-wide"><span>Why are you returning this?</span><textarea value={text} maxLength={2000} onChange={event => setText(event.target.value)}/></label>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions"><button className="ob-secondary" disabled={busy}>{busy ? "Sending…" : "Request a return"}</button></div>
  </form>;
}

function DecideForm({ onDecide }: { onDecide: (decision: "accept" | "decline", text: string) => Promise<void> }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"" | "accept" | "decline">("");
  const [error, setError] = useState("");
  async function choose(decision: "accept" | "decline") {
    if (busy) return;
    setBusy(decision);
    setError("");
    try {
      await onDecide(decision, text);
    } catch (err) {
      setError(err instanceof Error ? err.message : "The return could not be updated.");
    } finally {
      setBusy("");
    }
  }
  return <form className="ob-return-form" onSubmit={event => event.preventDefault()}>
    <label className="ob-field ob-field-wide"><span>Note (optional)</span><input value={text} maxLength={2000} onChange={event => setText(event.target.value)}/></label>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions">
      <button type="button" className="ob-primary" disabled={!!busy} onClick={() => void choose("accept")}>{busy === "accept" ? "Accepting…" : "Accept return"}</button>
      <button type="button" className="ob-secondary" disabled={!!busy} onClick={() => void choose("decline")}>{busy === "decline" ? "Declining…" : "Decline return"}</button>
    </div>
  </form>;
}

function ShipForm({ onShip }: { onShip: (shipment: { carrier: string; tracking: string; note: string }) => Promise<void> }) {
  const [carrierChoice, setCarrierChoice] = useState("USPS");
  const [customCarrier, setCustomCarrier] = useState("");
  const [tracking, setTracking] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const carrier = carrierChoice === "Other" ? customCarrier : carrierChoice;
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await onShip({ carrier, tracking, note });
    } catch (err) {
      setError(err instanceof Error ? err.message : "The return shipment could not be saved.");
    } finally {
      setBusy(false);
    }
  }
  return <form className="ob-return-form" onSubmit={event => void submit(event)} aria-label="Return shipment">
    <div className="ob-field-grid">
      <label className="ob-field"><span>Carrier</span><select value={carrierChoice} onChange={event => setCarrierChoice(event.target.value)}>{CARRIER_OPTIONS.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
      {carrierChoice === "Other" && <label className="ob-field"><span>Carrier name</span><input value={customCarrier} maxLength={60} onChange={event => setCustomCarrier(event.target.value)}/></label>}
      <label className="ob-field"><span>Tracking number</span><input value={tracking} maxLength={100} onChange={event => setTracking(event.target.value)}/></label>
      <label className="ob-field ob-field-wide"><span>Note (optional)</span><input value={note} maxLength={1000} onChange={event => setNote(event.target.value)}/></label>
    </div>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions"><button className="ob-secondary" disabled={busy}>{busy ? "Sending…" : "Add return tracking"}</button></div>
  </form>;
}
