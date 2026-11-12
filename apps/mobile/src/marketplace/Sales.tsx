import { useEffect, useRef, useState, type FormEvent } from "react";
import type { CurrencyCode, RateTable } from "../catalog";
import { EXTERNAL_TX_NOTE } from "../nodeWallet";
import { fetchSales, fulfillSale, type Sale } from "../seller";
import { useNodeResource } from "../useNodeResource";
import { sameOrderSnapshot, salesRefreshInterval } from "../orderList";
import { Empty, Money } from "./Marketplace";
import { OfferInbox } from "./OfferInbox";
import { PageHeading } from "./PageHeading";
import { OrderWorkspace } from "./OrderWorkspace";
import { InfoTip } from "./InfoTip";
import { CopyAddress, CopyButton } from "./CopyAddress";

/** Sales, polled while the node is a store. Calls `onNew` once for each sale that appears. */
export function useSales(enabled: boolean, onNew: (sale: Sale) => void, viewingSales = true) {
  const resource = useNodeResource(fetchSales, data => salesRefreshInterval(data, viewingSales), enabled, undefined, sameOrderSnapshot);
  const wasViewing = useRef(viewingSales);
  useEffect(() => {
    if (viewingSales && !wasViewing.current) void resource.refresh();
    wasViewing.current = viewingSales;
  }, [viewingSales, resource.refresh]);
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
  return <div className="ob-transaction-value"><code>{txid}</code><div className="ob-transaction-actions"><CopyButton value={txid} label="Copy transaction ID"/><InfoTip label="Transaction privacy">{EXTERNAL_TX_NOTE}</InfoTip></div></div>;
}

function deliverySummary(sale: Sale): string {
  const quantity = sale.lines.reduce((sum, line) => sum + line.quantity, 0);
  const delivery = sale.lines.some(line => line.kind === "physical") ? "Physical delivery" : "Digital delivery";
  return `${quantity} ${quantity === 1 ? "item" : "items"} · ${delivery}`;
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
  const physical = sale.lines.some(line => line.kind === "physical");
  const received = sale.zcash ? `${sale.zcash.receivedZec} / ${sale.zcash.amountZec} ZEC` : `${sale.receivedSats.toLocaleString("en-US")} / ${sale.totalSats.toLocaleString("en-US")} sats`;
  const confirmations = sale.confirmations ? ` · ${sale.confirmations} confirmation${sale.confirmations === 1 ? "" : "s"}` : "";
  return <div className="ob-order-expanded ob-sale-detail">
    <div className="ob-order-next"><span>Next step</span><h2>{sale.status === "Paid" ? physical ? "Ready to ship" : "Ready for digital delivery" : sale.status === "Shipped" ? "Awaiting buyer confirmation" : sale.status === "Completed" ? "Order complete" : ["Awaiting payment", "Payment seen"].includes(sale.status) ? "Awaiting confirmed payment" : sale.status}</h2><p>{sale.status === "Paid" ? "Payment is confirmed. Prepare the delivery details below." : sale.status === "Shipped" ? "The buyer can confirm receipt from their order." : sale.status === "Completed" ? "The buyer confirmed receipt. Delivery records remain available here." : ["Awaiting payment", "Payment seen"].includes(sale.status) ? "Keep this order on hold until the node confirms payment." : "Review the current order status before taking action."}</p></div>
    <div className="ob-order-detail-grid"><div className="ob-order-primary">
      {sale.status === "Paid" && <FulfillForm sale={sale} onDone={onChange}/>}
      {sale.fulfillment && <section className="ob-order-delivery"><h3>Delivery record</h3><p>{[sale.fulfillment.carrier, sale.fulfillment.tracking].filter(Boolean).join(" · ") || (physical ? "Shipment recorded" : "Delivered digitally")}</p>{sale.fulfillment.note && <p>{sale.fulfillment.note}</p>}{sale.fulfillment.content && <p className="ob-sale-content">{sale.fulfillment.content}</p>}</section>}
      {sale.zcash?.expired ? <p className="ob-form-error" role="alert">The Zcash quote for this sale ran out before it was paid in full, so the sale stays unpaid. Do not ship it. What arrives at the address is still counted above.</p> : ["Awaiting payment", "Payment seen"].includes(sale.status) && !sale.fulfillment ? <p className="ob-page-intro">{paymentSent(sale) ? "Payment sent, waiting for confirmation. " : ""}Fulfill once the payment confirms.</p> : null}
      <details className="ob-order-record"><summary>Payment & transaction records</summary><dl className="ob-sale-facts"><div><dt>Received</dt><dd>{received}{confirmations}</dd></div><div><dt>{sale.zcash ? "Zcash address" : "Payment address"}</dt><dd><CopyAddress value={sale.zcash?.address ?? sale.address} label={sale.zcash ? "Copy zcash address" : "Copy payment address"}/></dd></div>{sale.sentTxid && !sale.txids.includes(sale.sentTxid) && <div><dt>Buyer reported transaction</dt><dd><TxId txid={sale.sentTxid}/></dd></div>}{sale.txids.map(txid => <div key={txid}><dt>Transaction</dt><dd><TxId txid={txid}/></dd></div>)}</dl></details>
    </div><aside className="ob-order-facts-panel"><h3>Order summary</h3><ul>{sale.lines.map((line, index) => <li key={`${line.listingHash}-${index}`}><span>{line.title}</span><span>× {line.quantity}</span></li>)}</ul><dl><div><dt>Items</dt><dd><Money sats={sale.itemsSats} rates={rates} currency={currency}/></dd></div><div><dt>Shipping</dt><dd><Money sats={sale.shippingSats} rates={rates} currency={currency}/></dd></div><div><dt>Total</dt><dd><Money sats={sale.totalSats} rates={rates} currency={currency}/></dd></div></dl>{sale.shipTo && <section><h3>Ship to</h3><p className="ob-sale-address">{sale.shipTo}</p></section>}{sale.deliveryNote && <section><h3>Buyer’s note</h3><p>{sale.deliveryNote}</p></section>}<details className="ob-order-record"><summary>Order reference</summary><code>{sale.id}</code></details></aside></div>
  </div>;
}

export function Sales({ sales, rates, currency, onSeen }: { sales: SalesResource; rates: RateTable; currency: CurrencyCode; onSeen?: (list: Sale[]) => void }) {
  const [view, setView] = useState("orders");
  const list = sales.data ?? [];
  const seen = useRef(onSeen);
  seen.current = onSeen;
  useEffect(() => { if (sales.data) seen.current?.(sales.data); }, [sales.data]);
  return <section className="ob-orders ob-sales ob-order-center">
    <PageHeading title="Sales" description="Manage payments, fulfill orders and track completed deliveries." action={<button aria-label="Refresh sales" className="ob-secondary" disabled={sales.loading} onClick={() => void sales.refresh()}>Refresh</button>}/>
    <nav className="ob-sales-views" aria-label="Sales views"><button aria-current={view === "orders" ? "page" : undefined} onClick={() => setView("orders")}>Orders</button><button aria-current={view === "offers" ? "page" : undefined} onClick={() => setView("offers")}>Offers</button></nav>
    {sales.error && <p className="ob-form-error" role="alert">{sales.error}</p>}
    {view === "offers" ? <><h2 className="ob-offers-title">Offers</h2><OfferInbox showEmpty/></> : sales.loading && !sales.data ? <p role="status">Loading sales…</p> : !list.length ? (sales.error ? null : <Empty icon="paper" title="No sales yet" text="Orders will appear here when buyers purchase from your store."/>) : <OrderWorkspace seller rates={rates} currency={currency} rows={list.map(sale => ({ id: sale.id, title: sale.lines.map(line => line.title).join(", "), subtitle: "", amount: sale.totalSats, created: sale.created, attention: sale.status === "Paid", closed: sale.status === "Completed", status: sale.status === "Paid" ? sale.lines.some(line => line.kind === "physical") ? "Ready to ship" : "Ready to deliver" : sale.status, hint: deliverySummary(sale) }))} renderDetail={id => <SaleDetail sale={list.find(sale => sale.id === id)!} rates={rates} currency={currency} onChange={() => void sales.refresh()}/>}/>}
  </section>;
}
