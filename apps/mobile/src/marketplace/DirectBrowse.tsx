import { type FormEvent } from "react";
import { nodeRequired } from "../publicMarket";
import { PageHeading } from "./PageHeading";

/** The address box on the offline shop. The parent keeps `value` across a failed open. */
export function OpenStoreForm({ value, busy, onChange, onOpen }: { value: string; busy: boolean; onChange: (value: string) => void; onOpen: (pasted: string) => void }) {
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!busy && value.trim()) onOpen(value);
  }
  return <form className="ob-open-store" onSubmit={submit}>
    <label className="ob-field"><span>Store address</span><input aria-label="Store address" value={value} spellCheck={false} autoCapitalize="off" placeholder="http://….onion or https://…" onChange={(event) => onChange(event.target.value)}/></label>
    <button className="ob-primary" disabled={busy || !value.trim()}>{busy ? "Opening…" : "Open store"}</button>
    <p className="ob-meta">Listings are read from that seller's node and checked in this browser. Your bag stays on this device. Paying needs a wallet on your own node.</p>
  </form>;
}

function Gate({ kind, onBack, backLabel }: { kind: "buy" | "sell"; onBack?: () => void; backLabel?: string }) {
  const gate = nodeRequired(kind, typeof navigator === "undefined" ? "" : navigator.userAgent);
  return <section className="ob-checkout">
    {onBack && <button className="ob-back" type="button" onClick={onBack}>{backLabel}</button>}
    <PageHeading title={gate.title} description={gate.text}/>
    <div className="ob-empty-actions">
      <a className="ob-primary" href={gate.href}>Download OpenBazaar</a>
    </div>
    <p className="ob-meta">{gate.label}. Your bag stays on this device.</p>
  </section>;
}

/** Checkout while this page has no wallet. The bag itself is unchanged. */
export function DirectPay({ onBack, backLabel }: { onBack: () => void; backLabel: string }) {
  return <Gate kind="buy" onBack={onBack} backLabel={backLabel}/>;
}

/** Publishing a listing needs a node. The market stays read-only. */
export function SellGate() {
  return <Gate kind="sell"/>;
}

/** Shown above the catalog while listings come from the seller's node. */
export function DirectBanner({ url }: { url: string }) {
  return <div className="ob-network-badge" role="status">
    <span className="ob-status-dot" aria-hidden="true"/>
    Browsing {url}. Your bag stays on this device. Paying needs a wallet on your own node.
  </div>;
}
