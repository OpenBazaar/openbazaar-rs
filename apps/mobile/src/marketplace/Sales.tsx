import { useEffect, useRef, useState, type FormEvent } from "react";
import type { CurrencyCode, RateTable } from "../catalog";
import { EXTERNAL_TX_NOTE } from "../nodeWallet";
import { fetchSales, fulfillSale, type Sale } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { Empty, Icon, Money } from "./Marketplace";
import { OfferInbox } from "./OfferInbox";
import { PageHeading } from "./PageHeading";

const STEPS = ["Awaiting payment", "Payment seen", "Paid", "Shipped", "Completed"];

/** Sales, polled while the node is a store. Calls `onNew` once for each sale that appears. */
export function useSales(enabled: boolean, onNew: (sale: Sale) => void) {
  const resource = useNodeResource(fetchSales, 10000, enabled);
  const seen = useRef<Set<string> | null>(null);
  const notify = useRef(onNew);
  notify.current = onNew;
  useEffect(() => {
    const sales = resource.data;
    if (!sales) return;
    if (seen.current) for (const sale of sales) if (!seen.current.has(sale.id)) notify.current(sale);
    seen.current = new Set(sales.map(sale => sale.id));
  }, [resource.data]);
  return resource;
}
export type SalesResource = ReturnType<typeof useSales>;

/** The buyer says it paid, or the chain shows it unconfirmed; not paid until a block. */
function paymentSent(sale: Sale): boolean {
  return sale.status === "Payment seen" || (sale.status === "Awaiting payment" && !!sale.sentTxid);
}

function TxId({ txid }: { txid: string }) {
  return <code>{txid}</code>;
}

function Progress({ status }: { status: string }) {
  const at = STEPS.indexOf(status);
  return <ol className="ob-sale-progress" aria-label={`Status: ${status}`}>{STEPS.map((step, i) => <li key={step} className={i < at ? "is-done" : i === at ? "is-current" : ""}>{step}</li>)}</ol>;
}

function FulfillForm({ sale, onDone }: { sale: Sale; onDone: (sale: Sale) => void }) {
  const digital = sale.lines.some(line => line.kind === "digital");
  const physical = sale.lines.some(line => line.kind === "physical");
  const [carrier, setCarrier] = useState(""), [tracking, setTracking] = useState(""), [note, setNote] = useState(""), [content, setContent] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (digital && !physical && !content.trim()) {
      setError("Add the download link, key, or file details the buyer needs.");
      return;
    }
    setBusy(true);
    setError("");
    try {
      onDone(await fulfillSale(sale.id, { carrier, tracking, note, content }));
    } catch (err) {
      setError(err instanceof Error ? err.message : "The order could not be marked as sent.");
    } finally {
      setBusy(false);
    }
  }
  return <form className="ob-fulfill" onSubmit={e => void submit(e)} aria-label="Mark as sent">
    <h3>{physical ? "Ship this order" : "Deliver this order"}</h3>
    <div className="ob-field-grid">
      {physical && <label className="ob-field"><span>Carrier</span><input value={carrier} maxLength={60} placeholder="USPS" onChange={e => setCarrier(e.target.value)}/></label>}
      {physical && <label className="ob-field"><span>Tracking number</span><input value={tracking} maxLength={100} onChange={e => setTracking(e.target.value)}/></label>}
      {digital && <label className="ob-field ob-field-wide"><span>Delivery for the buyer</span><textarea value={content} maxLength={4000} placeholder="Download link, license key, or instructions" onChange={e => setContent(e.target.value)}/></label>}
      <label className="ob-field ob-field-wide"><span>Note to the buyer (optional)</span><input value={note} maxLength={1000} onChange={e => setNote(e.target.value)}/></label>
    </div>
    {error && <p className="ob-form-error" role="alert">{error}</p>}
    <div className="ob-form-actions"><button className="ob-primary" disabled={busy}>{busy ? "Sending…" : physical ? "Mark as shipped" : "Send to buyer"}</button></div>
  </form>;
}

function SaleDetail({ sale, rates, currency, onChange }: { sale: Sale; rates: RateTable; currency: CurrencyCode; onChange: (sale: Sale) => void }) {
  return <div className="ob-order-expanded ob-sale-detail">
    <Progress status={sale.status}/>
    <dl className="ob-sale-facts">
      <div><dt>Items</dt><dd>{sale.lines.map(line => <span key={line.listingHash}>{line.quantity} × {line.title}</span>)}</dd></div>
      <div><dt>Items total</dt><dd><Money sats={sale.itemsSats} rates={rates} currency={currency}/></dd></div>
      <div><dt>Shipping</dt><dd><Money sats={sale.shippingSats} rates={rates} currency={currency}/></dd></div>
      <div><dt>Received</dt><dd>{sale.receivedSats.toLocaleString("en-US")} of {sale.totalSats.toLocaleString("en-US")} sats{sale.confirmations ? ` · ${sale.confirmations} confirmation${sale.confirmations === 1 ? "" : "s"}` : ""}</dd></div>
      {sale.shipTo && <div><dt>Ship to</dt><dd className="ob-sale-address">{sale.shipTo}</dd></div>}
      {sale.deliveryNote && <div><dt>Buyer's note</dt><dd>{sale.deliveryNote}</dd></div>}
      <div><dt>Payment address</dt><dd><code>{sale.address}</code></dd></div>
      {sale.sentTxid && !sale.txids.includes(sale.sentTxid) && <div><dt>Buyer sent</dt><dd><TxId txid={sale.sentTxid}/></dd></div>}
      {sale.txids.map(txid => <div key={txid}><dt>Transaction</dt><dd><TxId txid={txid}/></dd></div>)}
      {(sale.sentTxid || sale.txids.length > 0) && <div><dt>Viewing it</dt><dd>{EXTERNAL_TX_NOTE}</dd></div>}
      {sale.fulfillment && <div><dt>Sent</dt><dd>{[sale.fulfillment.carrier, sale.fulfillment.tracking].filter(Boolean).join(" · ") || "Delivered"}{sale.fulfillment.note && <p>{sale.fulfillment.note}</p>}{sale.fulfillment.content && <p className="ob-sale-content">{sale.fulfillment.content}</p>}</dd></div>}
    </dl>
    {sale.status === "Paid" ? <FulfillForm sale={sale} onDone={onChange}/> : sale.status === "Awaiting payment" || sale.status === "Payment seen" ? <p className="ob-page-intro">{paymentSent(sale) && "Payment sent, waiting for confirmation. "}Ship once the payment confirms. Your node watches the address and tells the buyer.</p> : sale.status === "Shipped" ? <p className="ob-page-intro">Waiting for the buyer to confirm it arrived.</p> : <p className="ob-page-intro">The buyer confirmed this order. It is complete.</p>}
  </div>;
}

export function Sales({ sales, rates, currency }: { sales: SalesResource; rates: RateTable; currency: CurrencyCode }) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const list = sales.data ?? [];
  return <section className="ob-orders ob-sales">
    <PageHeading title="Sales" description="Orders buyers placed with your store, paid straight to your node's wallet." action={<button className="ob-secondary" disabled={sales.loading} onClick={() => void sales.refresh()}>Refresh sales</button>}/>
    {sales.error && <p className="ob-form-error" role="alert">{sales.error}</p>}
    <OfferInbox/>
    {sales.loading && !sales.data ? <p role="status">Loading sales…</p> : !list.length ? <Empty icon="paper" title="No sales yet" text="When a buyer orders from your store, the order appears here."/> : list.map(sale => <article className={`ob-order-row ${sale.status === "Paid" ? "needs-action" : ""}`} key={sale.id}>
      <button className="ob-order-toggle" onClick={() => setExpanded(expanded === sale.id ? null : sale.id)} aria-expanded={expanded === sale.id} aria-label={`Sale ${sale.lines.map(l => l.title).join(", ")}`}>
        <span className="ob-order-icon"><Icon name="paper"/></span>
        <span className="ob-order-name"><strong>{sale.lines.map(l => `${l.quantity > 1 ? `${l.quantity} × ` : ""}${l.title}`).join(", ")}</strong><span>{new Date(sale.created * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</span></span>
        <span className="ob-order-status">{sale.status === "Paid" ? "Paid · ready to ship" : paymentSent(sale) ? "Payment sent, waiting for confirmation" : sale.status}</span>
        <Money sats={sale.totalSats} rates={rates} currency={currency}/>
        <Icon name="chevron" size={16}/>
      </button>
      {expanded === sale.id && <SaleDetail sale={sale} rates={rates} currency={currency} onChange={() => void sales.refresh()}/>}
    </article>)}
  </section>;
}
