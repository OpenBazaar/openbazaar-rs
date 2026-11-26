import { useRef, useState } from "react";
import type { BuyerOrder, CurrencyCode, RateTable } from "../catalog";
import { fetchOrders, onThisComputer, shopPairingMessage } from "../catalog";
import { pairingKind, pairingOrdersText } from "../pairingUi";
import { ordersSettled } from "../orderState";
import { orderTime, sameOrderSnapshot } from "../orderList";
import { useNodeResource } from "../useNodeResource";
import { Empty, Icon, Money, Payment } from "./Marketplace";
import { PairButton } from "./PairingDialog";
import { PageHeading } from "./PageHeading";
import { RemoteOrderStatus } from "./RemoteOrder";
import { returnStateLabel } from "../returnCase";
import { OrderWorkspace, type OrderWorkspaceState } from "./OrderWorkspace";

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

function orderRailLabel(order: BuyerOrder): string | undefined {
  if (order.payment?.ln) return "Lightning";
  if (order.payment?.rail === "xmr") return "Monero";
  if (order.payment?.rail === "zec") return "Zcash";
  if (order.payment || order.settlement === "escrow") return "Bitcoin";
  return undefined;
}

function orderNeedsAttention(order: BuyerOrder): boolean {
  return ["Awaiting payment", "Shipped"].includes(order.status)
    || !!order.payError
    || !!order.remote?.oversold
    || order.remote?.returnCase?.state === "requested";
}

function PaymentRecord({ order }: { order: BuyerOrder }) {
  const settled = ["Paid", "Shipped", "Completed"].includes(order.status);
  // An order opened while awaiting payment stays visible as confirmation arrives.
  const [open, setOpen] = useState(!settled);
  return <details className={settled ? "ob-order-record" : "ob-order-live-payment"} open={!settled || open} onToggle={event => { if (settled) setOpen(event.currentTarget.open); }}><summary hidden={!settled}>Payment record</summary><Payment receipt={order} hideStatus/></details>;
}

function PurchaseDetail({ order, rates, currency, onChange }: { order: BuyerOrder; rates: RateTable; currency: CurrencyCode; onChange: () => void }) {
  const settled = ["Paid", "Shipped", "Completed"].includes(order.status);
  // Keep payment first when an order is opened while still awaiting payment.
  const [paymentFirst] = useState(!settled);
  const showPayment = !order.remote || !!order.payment;
  const payment = showPayment ? <PaymentRecord order={order}/> : null;
  const date = new Date(orderTime(order.created));
  const rail = orderRailLabel(order);
  const title = order.lines.map(line => line.title).join(", ") || `Order #${order.id.slice(0, 8)}`;
  const attention = orderNeedsAttention(order);
  return (
    <div className="ob-order-detail-body ob-purchase-detail">
      <header className="ob-order-detail-header">
        <div className="ob-order-detail-title">
          <div className="ob-order-detail-meta">
            <span>#{order.id.slice(0, 8)}</span>
            <span aria-hidden="true">·</span>
            <time dateTime={date.toISOString()}>{date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</time>
            {rail && <><span aria-hidden="true">·</span><span>{rail}</span></>}
            {order.seller && <><span aria-hidden="true">·</span><span>{order.seller}</span></>}
          </div>
          <h1>{title}</h1>
        </div>
        <div className="ob-order-detail-summary-meta">
          <span className={`ob-order-status ${attention ? "is-actionable" : ""}`}>{order.status}</span>
          <Money sats={order.totalSats} rates={rates} currency={currency}/>
        </div>
      </header>
      <div className="ob-order-next">
        <h2>{order.status}</h2>
        <p>{nextStep(order)}</p>
      </div>
      <div className="ob-order-detail-grid">
        <div className="ob-order-primary">
          {paymentFirst && payment}
          <RemoteOrderStatus order={order} onChange={onChange}/>
          {!paymentFirst && payment}
        </div>
        <aside className="ob-order-facts-panel">
          <h3>Order summary</h3>
          <ul>{order.lines.map((line, index) => <li key={`${line.listingHash}-${index}`}><span>{line.title}{line.variantLabel && <small>{line.variantLabel}</small>}</span><span>× {line.quantity}</span></li>)}</ul>
          <dl>
            <div><dt>Items</dt><dd><Money sats={order.itemsSats} rates={rates} currency={currency}/></dd></div>
            <div><dt>Shipping</dt><dd><Money sats={order.shippingSats} rates={rates} currency={currency}/></dd></div>
            <div><dt>Total</dt><dd><Money sats={order.totalSats} rates={rates} currency={currency}/></dd></div>
          </dl>
          <details className="ob-order-record"><summary>Order reference</summary><code>{order.id}</code></details>
        </aside>
      </div>
    </div>
  );
}

type OrdersProps = {
  rates: RateTable;
  currency: CurrencyCode;
  onBrowse: () => void;
  selectedOrderId?: string | null;
  unreadByOrder?: Record<string, number>;
  onSelectOrder: (id: string) => void;
  onBack: () => void;
};

export function Orders({ rates, currency, onBrowse, selectedOrderId = null, unreadByOrder = {}, onSelectOrder, onBack }: OrdersProps) {
  const resource = useNodeResource(fetchOrders, 12000, true, ordersSettled, sameOrderSnapshot);
  const orders = resource.data ?? [];
  const listState = useRef<OrderWorkspaceState | null>(null);

  if (selectedOrderId) {
    const order = orders.find(item => item.id === selectedOrderId) ?? null;
    return (
      <section className="ob-orders ob-order-center ob-order-detail-page">
        <button type="button" className="ob-back ob-order-back" onClick={onBack}>
          <Icon name="arrow" size={16}/>Back to orders
        </button>
        {resource.error && (
          shopPairingMessage(resource.error)
            ? <Empty icon="bag" title="Pair this browser to see your orders" text={pairingOrdersText(pairingKind(onThisComputer()))}><PairButton/></Empty>
            : <p className="ob-form-error" role="alert">{resource.error}</p>
        )}
        {resource.loading && !resource.data
          ? <p role="status">Loading order…</p>
          : !resource.data
            ? null
            : !order
              ? <Empty icon="bag" title="Order not found" text="This order could not be found in your order history."><button type="button" className="ob-primary" onClick={onBack}>Back to orders</button></Empty>
              : <PurchaseDetail key={order.id} order={order} rates={rates} currency={currency} onChange={() => void resource.refresh()}/>}
      </section>
    );
  }

  return (
    <section className="ob-orders ob-order-center">
      <PageHeading title="Your orders" description="Payments, deliveries and the next step for each purchase." action={<button aria-label="Refresh orders" className="ob-secondary" disabled={resource.loading} onClick={() => void resource.refresh()}>Refresh</button>}/>
      {resource.error && (shopPairingMessage(resource.error) ? <Empty icon="bag" title="Pair this browser to see your orders" text={pairingOrdersText(pairingKind(onThisComputer()))}><PairButton/></Empty> : <p className="ob-form-error" role="alert">{resource.error}</p>)}
      {resource.loading && !resource.data ? <p role="status">Loading orders…</p> : !resource.data ? null : !orders.length ? <Empty icon="bag" title="No orders yet" text="Orders appear here after checkout."><button className="ob-primary" onClick={onBrowse}>Browse products</button></Empty> : (
        <OrderWorkspace
          rates={rates}
          currency={currency}
          onSelect={onSelectOrder}
          savedStateRef={listState}
          rows={orders.map(order => ({
            id: order.id,
            title: order.lines.map(line => line.title).join(", "),
            subtitle: order.seller,
            created: order.created,
            amount: order.totalSats,
            status: order.status,
            rail: orderRailLabel(order),
            hint: order.remote?.returnCase ? `${nextStep(order)} ${returnStateLabel(order.remote.returnCase.state)}.` : nextStep(order),
            attention: orderNeedsAttention(order),
            closed: ["Completed", "Declined by the seller"].includes(order.status),
            messageCount: order.remote?.messages.length ?? 0,
            unreadMessages: unreadByOrder[order.id] ?? 0,
          }))}
        />
      )}
    </section>
  );
}
