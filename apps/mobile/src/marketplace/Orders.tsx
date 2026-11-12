import { useState } from "react";
import type { BuyerOrder, CurrencyCode, RateTable } from "../catalog";
import { fetchOrders, onThisComputer, shopPairingMessage } from "../catalog";
import { pairingKind, pairingOrdersText } from "../pairingUi";
import { ordersSettled } from "../orderState";
import { sameOrderSnapshot } from "../orderList";
import { useNodeResource } from "../useNodeResource";
import { Empty, Money, Payment } from "./Marketplace";
import { PairButton } from "./PairingDialog";
import { PageHeading } from "./PageHeading";
import { RemoteOrderStatus } from "./RemoteOrder";
import { OrderWorkspace } from "./OrderWorkspace";

function nextStep(order: BuyerOrder): string {
  if (order.remote?.oversold) return "The seller reported a stock issue. Review the order before taking further action.";
  if (order.payError) return "Your payment needs attention. Review the payment details below.";
  if (order.settlement === "escrow") return "This order uses escrow. Review its terms and current settlement status below.";
  if (order.status === "Awaiting payment") return "Complete payment to continue this order. Each seller is paid separately.";
  if (order.status === "Payment seen") return "Payment detected. Waiting for the required confirmations.";
  if (order.status === "Paid") return "Payment confirmed. Waiting for the seller to fulfill your order.";
  if (order.status === "Shipped") return order.remote?.fulfillment?.content ? "Your digital delivery is ready. Review it before confirming receipt." : "Your order is on its way. Confirm receipt after it arrives.";
  if (order.status === "Completed") return "This order is complete. Your delivery and payment records are below.";
  if (order.status === "Awaiting seller") return "Waiting for the seller to accept and provide payment terms.";
  return "Review the latest seller response and order status below.";
}

function PaymentRecord({ order }: { order: BuyerOrder }) {
  const settled = ["Paid", "Shipped", "Completed"].includes(order.status);
  // An order opened while awaiting payment stays visible as confirmation arrives.
  const [open, setOpen] = useState(!settled);
  return <details className={settled ? "ob-order-record" : "ob-order-live-payment"} open={!settled || open} onToggle={event => { if (settled) setOpen(event.currentTarget.open); }}><summary hidden={!settled}>Payment record</summary><Payment receipt={order}/></details>;
}
export function Orders({ rates, currency, onBrowse }: { rates: RateTable; currency: CurrencyCode; onBrowse: () => void }) {
  const resource = useNodeResource(fetchOrders, 12000, true, ordersSettled, sameOrderSnapshot);
  const orders = resource.data ?? [];
  return <section className="ob-orders ob-order-center"><PageHeading title="Your orders" description="Payments, deliveries and the next step for each purchase." action={<button aria-label="Refresh orders" className="ob-secondary" disabled={resource.loading} onClick={() => void resource.refresh()}>Refresh</button>}/>
    {resource.error && (shopPairingMessage(resource.error) ? <Empty icon="bag" title="Pair this browser to see your orders" text={pairingOrdersText(pairingKind(onThisComputer()))}><PairButton/></Empty> : <p className="ob-form-error" role="alert">{resource.error}</p>)}
    {resource.loading && !resource.data ? <p role="status">Loading orders…</p> : !resource.data ? null : !orders.length ? <Empty icon="bag" title="No orders yet" text="Orders appear here after checkout."><button className="ob-primary" onClick={onBrowse}>Browse products</button></Empty> : <OrderWorkspace rates={rates} currency={currency} rows={orders.map(order => ({ id: order.id, title: order.lines.map(line => line.title).join(", "), subtitle: order.seller, created: order.created, amount: order.totalSats, status: order.status, hint: nextStep(order), attention: ["Awaiting payment", "Shipped"].includes(order.status) || !!order.payError || !!order.remote?.oversold, closed: ["Completed", "Declined by the seller"].includes(order.status) }))} renderDetail={id => {
      const order = orders.find(item => item.id === id)!;
      return <div className="ob-order-expanded ob-purchase-detail"><div className="ob-order-next"><span>Order status</span><h2>{order.status}</h2><p>{nextStep(order)}</p></div><div className="ob-order-detail-grid"><div className="ob-order-primary"><RemoteOrderStatus order={order} onChange={() => void resource.refresh()}/>{(!order.remote || order.payment) && <PaymentRecord order={order}/>}</div><aside className="ob-order-facts-panel"><h3>Order summary</h3><ul>{order.lines.map((line, index) => <li key={`${line.listingHash}-${index}`}><span>{line.title}{line.variantLabel && <small>{line.variantLabel}</small>}</span><span>× {line.quantity}</span></li>)}</ul><dl><div><dt>Items</dt><dd><Money sats={order.itemsSats} rates={rates} currency={currency}/></dd></div><div><dt>Shipping</dt><dd><Money sats={order.shippingSats} rates={rates} currency={currency}/></dd></div><div><dt>Total</dt><dd><Money sats={order.totalSats} rates={rates} currency={currency}/></dd></div></dl><details className="ob-order-record"><summary>Order reference</summary><code>{order.id}</code></details></aside></div></div>;
    }}/>} </section>;
}
