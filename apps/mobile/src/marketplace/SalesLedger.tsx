import type { ReactNode } from 'react';
import type { CurrencyCode, RateTable } from '../catalog';
import { orderTime, type OrderListRow } from '../orderList';
import { Money } from './Marketplace';
import { Dialog } from './WalletLock';

type Props = {
  rows: OrderListRow[]; allRows: OrderListRow[]; selected: string | null;
  rates: RateTable; currency: CurrencyCode; onSelect: (id: string) => void;
  onClose: () => void; renderDetail: (id: string) => ReactNode;
};

export function SalesLedger({ rows, allRows, selected, rates, currency, onSelect, onClose, renderDetail }: Props) {
  const order = allRows.find(row => row.id === selected);
  return <>
    {rows.length > 0 && <div className="ob-sales-ledger"><table className="ob-sales-table">
      <caption className="ob-sr-only">Store orders</caption>
      <thead><tr><th scope="col">Order / items</th><th scope="col">Placed</th><th scope="col">Payment</th><th scope="col">Fulfillment</th><th scope="col">Total</th><th scope="col"><span className="ob-sr-only">Action</span></th></tr></thead>
      <tbody>{rows.map(row => {
        const date = new Date(orderTime(row.created));
        const action = row.attention ? row.deliveryKind === 'digital' ? 'Deliver' : 'Ship' : 'View';
        return <tr className={`ob-order-row ${row.attention ? 'needs-action' : ''} ${selected === row.id ? 'is-selected' : ''}`} key={row.id}>
        <td className="ob-sales-item"><span className="ob-sales-reference">#{row.id.slice(0, 8)}<small>{row.hint}</small><time className="ob-sales-compact-date" dateTime={date.toISOString()} title={date.toLocaleDateString()}>{date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: '2-digit' })}</time></span><strong title={row.itemSummary ?? row.title}>{row.itemSummary ?? row.title}</strong></td>
        <td className="ob-sales-placed" data-label="Placed"><time dateTime={new Date(orderTime(row.created)).toISOString()}>{new Date(orderTime(row.created)).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })}</time></td>
        <td className="ob-sales-payment-cell" data-label="Payment"><span className={`ob-sales-payment ${row.awaitingPayment ? 'is-pending' : ''}`}>{row.paymentStatus}</span></td>
        <td className="ob-sales-state-cell" data-label="Fulfillment"><span className="ob-sales-compact-payment"><span className="ob-sr-only">Payment: </span>{row.paymentStatus}<span className="ob-sales-state-separator" aria-hidden="true">·</span></span><span className={`ob-order-status ${row.attention ? 'is-actionable' : ''}`}>{row.fulfillmentStatus}</span></td>
        <td className="ob-sales-total" data-label="Total"><Money sats={row.amount} rates={rates} currency={currency}/></td>
        <td className="ob-sales-action"><button type="button" className={row.attention ? 'ob-sales-fulfill' : 'ob-sales-view'} aria-label={`${action} order: ${row.title}`} aria-haspopup="dialog" onClick={() => onSelect(row.id)}>{action}<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m6 3 5 5-5 5"/></svg></button></td>
      </tr>})}</tbody>
    </table></div>}
    {order && <Dialog key={order.id} label={`Order details ${order.id.slice(0, 8)}`} className="ob-sales-detail-dialog" onCancel={onClose}>
      <header className="ob-sales-detail-header"><div><span>Order #{order.id.slice(0, 8)}</span><p>{new Date(orderTime(order.created)).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}</p></div><button type="button" className="ob-sales-close" onClick={onClose}>Back to sales <span aria-hidden="true">×</span></button></header>
      {renderDetail(order.id)}
    </Dialog>}
  </>;
}
