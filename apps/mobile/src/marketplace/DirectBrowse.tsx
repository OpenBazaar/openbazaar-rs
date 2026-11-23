import { type FormEvent } from "react";
import { PAYMENT_NOTE } from "../directStore";
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

/** Checkout while this page has no wallet. The bag itself is unchanged. */
export function DirectPay({ onBack, backLabel }: { onBack: () => void; backLabel: string }) {
  return <section className="ob-checkout">
    <button className="ob-back" type="button" onClick={onBack}>{backLabel}</button>
    <PageHeading title="Paying needs a wallet" description={PAYMENT_NOTE}/>
  </section>;
}

/** Shown above the catalog while listings come from the seller's node. */
export function DirectBanner({ url }: { url: string }) {
  return <div className="ob-network-badge" role="status">
    <span className="ob-status-dot" aria-hidden="true"/>
    Browsing {url}. Your bag stays on this device. Paying needs a wallet on your own node.
  </div>;
}
