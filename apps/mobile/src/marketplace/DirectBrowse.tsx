import { type FormEvent } from "react";
import type { ListingHeader } from "../catalog";
import { nodeRequired } from "../publicMarket";
import { PageHeading } from "./PageHeading";
import { TorCheckout } from "./TorCheckout";

/** The address box on the offline shop. The parent keeps `value` across a failed open. */
export function OpenStoreForm({ value, busy, onChange, onOpen }: { value: string; busy: boolean; onChange: (value: string) => void; onOpen: (pasted: string) => void }) {
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!busy && value.trim()) onOpen(value);
  }
  return <form className="ob-open-store" onSubmit={submit}>
    <label className="ob-field"><span>Store address</span><input aria-label="Store address" value={value} spellCheck={false} autoCapitalize="off" placeholder="http://….onion or https://…" onChange={(event) => onChange(event.target.value)}/></label>
    <button className="ob-primary" disabled={busy || !value.trim()}>{busy ? "Opening…" : "Open store"}</button>
    <p className="ob-meta">Listings are read from that seller's node and checked in this browser. Your bag stays on this device.</p>
  </form>;
}

function EscrowGate({ onBack, backLabel }: { onBack?: () => void; backLabel?: string }) {
  return <section className="ob-checkout">
    {onBack && <button className="ob-back" type="button" onClick={onBack}>{backLabel}</button>}
    <PageHeading title="2-of-3 escrow requires the OpenBazaar app" description="Escrow key management cannot run safely in a seller-served web page. Download the app for moderated escrow orders."/>
    <div className="ob-empty-actions">
      <a className="ob-primary" href={nodeRequired("buy", typeof navigator === "undefined" ? "" : navigator.userAgent).href}>Download OpenBazaar</a>
    </div>
  </section>;
}

function DownloadGate({ onBack, backLabel }: { onBack?: () => void; backLabel?: string }) {
  const gate = nodeRequired("buy", typeof navigator === "undefined" ? "" : navigator.userAgent);
  return <section className="ob-checkout">
    {onBack && <button className="ob-back" type="button" onClick={onBack}>{backLabel}</button>}
    <PageHeading title={gate.title} description={gate.text}/>
    <div className="ob-empty-actions">
      <a className="ob-primary" href={gate.href}>Download OpenBazaar</a>
    </div>
    <p className="ob-meta">{gate.label}. Your bag stays on this device.</p>
  </section>;
}

/**
 * Direct-pay checkout in Tor Browser. When the listing supports direct pay,
 * renders the inline TorCheckout flow. When 2-of-3 moderated escrow is
 * required, shows an honest gate explaining why the app is needed.
 */
/**
 * Direct-pay checkout in Tor Browser. When a listing and vendor URL are
 * provided and the settlement is direct pay, renders the inline TorCheckout
 * flow. When 2-of-3 moderated escrow is required, shows an honest gate.
 */
export function DirectPay({ listing, vendorUrl, settlement, onBack, backLabel }: {
  listing?: ListingHeader;
  vendorUrl?: string;
  settlement?: "direct" | "escrow";
  onBack: () => void;
  backLabel: string;
}) {
  if (settlement === "escrow") {
    return <EscrowGate onBack={onBack} backLabel={backLabel}/>;
  }
  if (listing && vendorUrl) {
    return <TorCheckout listing={listing} vendorUrl={vendorUrl} onBack={onBack} backLabel={backLabel}/>;
  }
  return <DownloadGate onBack={onBack} backLabel={backLabel}/>;
}

/** Publishing a listing needs a node. The market stays read-only. */
export function SellGate() {
  return <DownloadGate/>;
}

/** Shown above the catalog while listings come from the seller's node. */
export function DirectBanner({ url }: { url: string }) {
  return <div className="ob-network-badge" role="status">
    <span className="ob-status-dot" aria-hidden="true"/>
    Browsing {url}. Your bag stays on this device.
  </div>;
}
