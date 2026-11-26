import { useLayoutEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import type { CurrencyCode, RateTable } from "../catalog";
import { indexOrders, orderPage, orderTime, selectOrders, type OrderFilter, type OrderListRow, type OrderQuery, type OrderSort, type OrderRange } from "../orderList";
import { Money } from "./Marketplace";
import { SalesLedger } from "./SalesLedger";

export type OrderWorkspaceState = {
  query: OrderQuery;
  page: number;
  size: number;
  focusId: string | null;
};

export function OrderMessageBadge({ count = 0, unread = 0 }: { count?: number; unread?: number }) {
  if (count <= 0 && unread <= 0) return null;
  const hasUnread = unread > 0;
  const label = hasUnread
    ? (unread === 1 ? "New message" : `${unread} unread`)
    : (count === 1 ? "1 message" : `${count} messages`);
  const title = hasUnread && count > unread ? `${unread} unread of ${count} messages` : undefined;
  return (
    <span className={`ob-order-msg-badge${hasUnread ? " is-unread" : ""}`} title={title}>
      {hasUnread && <span className="ob-order-msg-dot" aria-hidden="true"/>}
      <svg width={13} height={13} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
      <span>{label}</span>
    </span>
  );
}

type Props = { rows: OrderListRow[]; rates: RateTable; currency: CurrencyCode; onSelect: (id: string) => void; seller?: boolean; savedStateRef?: MutableRefObject<OrderWorkspaceState | null> };
export function OrderWorkspace({ rows, rates, currency, onSelect, seller = false, savedStateRef }: Props) {
  const [query, setQuery] = useState<OrderQuery>(() => savedStateRef?.current?.query ?? ({ filter: seller && rows.some(row => row.attention) ? "attention" : "all", search: "", sort: "priority", range: "all", from: "", to: "" }));
  const [page, setPage] = useState(() => savedStateRef?.current?.page ?? 0);
  const [size, setSize] = useState(() => savedStateRef?.current?.size ?? 25);
  const [focusId, setFocusId] = useState<string | null>(() => savedStateRef?.current?.focusId ?? null);
  const top = useRef<HTMLDivElement>(null);
  const queue = useRef<HTMLDivElement>(null);
  const indexed = useMemo(() => indexOrders(rows), [rows]);
  const selected = useMemo(() => selectOrders(indexed, query), [indexed, query]);
  const visible = orderPage(selected.rows, page, size, focusId);
  if (savedStateRef) savedStateRef.current = { query, page: visible.page, size, focusId };
  const noun = seller ? "sales" : "orders";
  const countNoun = selected.rows.length === 1 ? (seller ? "sale" : "order") : noun;
  const invalidDates = query.range === "custom" && query.from && query.to && query.from > query.to;
  const filters: [OrderFilter, string][] = seller ? [["attention", "To fulfill"], ["awaiting", "Awaiting payment"], ["progress", "In progress"], ["closed", "Completed"], ["all", "All"]] : [["all", "All"], ["attention", "Needs attention"], ["progress", "In progress"], ["closed", "Closed"]];
  useLayoutEffect(() => {
    const group = queue.current;
    if (!seller || !group) return;
    const reveal = () => {
      const active = group.querySelector('button[aria-pressed="true"]');
      if (!active) return;
      const edge = group.getBoundingClientRect(), item = active.getBoundingClientRect();
      if (item.left < edge.left) group.scrollLeft -= edge.left - item.left;
      else if (item.right > edge.right) group.scrollLeft += item.right - edge.right;
    };
    reveal();
    const observer = new ResizeObserver(reveal);
    observer.observe(group);
    return () => observer.disconnect();
  }, [seller, query.filter]);
  function startOfList() {
    if (seller) top.current?.closest('.ob-content')?.scrollTo({ top: 0, behavior: 'instant' });
  }
  function changeQuery(change: Partial<OrderQuery>) {
    setQuery(previous => ({ ...previous, ...change }));
    setPage(0);
    setFocusId(null);
    startOfList();
  }
  function changePage(next: number, fromBottom = false) {
    setPage(next);
    setFocusId(null);
    startOfList();
    if (fromBottom) {
      if (!seller) top.current?.scrollIntoView({ block: "start" });
      top.current?.focus({ preventScroll: true });
    }
  }
  function selectRow(id: string) {
    if (savedStateRef) savedStateRef.current = { query, page: visible.page, size, focusId: id };
    onSelect(id);
  }
  function pagination(bottom = false) {
    return <nav className="ob-order-pagination" aria-label={`${seller ? "Sales" : "Orders"} pagination${bottom ? " bottom" : ""}`}>
      <button aria-label="First page" disabled={visible.page === 0} onClick={() => changePage(0, bottom)}>«</button>
      <button disabled={visible.page === 0} onClick={() => changePage(visible.page - 1, bottom)}>Previous</button>
      <span>Page {visible.page + 1} of {visible.pages.toLocaleString()}</span>
      <button disabled={visible.page + 1 >= visible.pages} onClick={() => changePage(visible.page + 1, bottom)}>Next</button>
      <button aria-label="Last page" disabled={visible.page + 1 >= visible.pages} onClick={() => changePage(visible.pages - 1, bottom)}>»</button>
    </nav>;
  }
  return <>
    <div className={seller ? "ob-sales-toolbar" : undefined}>
    <div className="ob-order-controls" ref={top} tabIndex={-1}>
      <div ref={queue} role="group" aria-label={seller ? "Filter sales" : "Filter orders"}>{filters.map(([key, label]) => <button key={key} aria-pressed={query.filter === key} onClick={() => changeQuery({ filter: key })}>{label}<span>{selected.counts[key].toLocaleString()}</span></button>)}</div>
      <input type="search" aria-label={seller ? "Search sales" : "Search orders"} placeholder={seller ? "Search product or order ID" : "Search product, seller or order ID"} value={query.search} onChange={event => changeQuery({ search: event.target.value })}/>
    </div>
    </div>
    <div className="ob-order-refine">
      <label>Sort by<select aria-label={`Sort ${noun}`} value={query.sort} onChange={event => changeQuery({ sort: event.target.value as OrderSort })}><option value="priority">Action needed, oldest first</option><option value="newest">Newest first</option><option value="oldest">Oldest first</option><option value="highest">Highest total</option><option value="lowest">Lowest total</option></select></label>
      <label>Order date<select aria-label="Order date" value={query.range} onChange={event => changeQuery({ range: event.target.value as OrderRange })}><option value="all">Any time</option><option value="7">Last 7 days</option><option value="30">Last 30 days</option><option value="90">Last 90 days</option><option value="custom">Custom range</option></select></label>
      <label>Per page<select aria-label="Orders per page" value={size} onChange={event => { setSize(Number(event.target.value)); setPage(0); setFocusId(null); startOfList(); }}><option value={25}>25</option><option value={50}>50</option></select></label>
      {query.range === "custom" && <><label>From<input type="date" aria-label="From date" value={query.from} onChange={event => changeQuery({ from: event.target.value })}/></label><label>Through<input type="date" aria-label="Through date" min={query.from || undefined} value={query.to} onChange={event => changeQuery({ to: event.target.value })}/></label></>}
      {(query.search || query.range !== "all") && <button className="ob-text-link" onClick={() => changeQuery({ search: "", range: "all", from: "", to: "" })}>Reset search & dates</button>}
    </div>
    {invalidDates && <p className="ob-form-error" role="alert">The end date must be on or after the start date.</p>}
    <div className="ob-order-list-meta">
      <p role="status" aria-live="polite">{selected.rows.length ? `${(visible.start + 1).toLocaleString()}–${Math.min(visible.start + size, selected.rows.length).toLocaleString()} of ${selected.rows.length.toLocaleString()} ${countNoun}` : `0 ${noun}`}</p>
      {selected.rows.length > size && pagination()}
    </div>
    {!selected.rows.length && <div className="ob-order-no-results"><h2>{seller && query.filter === "attention" && !query.search && query.range === "all" ? "No orders to fulfill" : `No matching ${noun}`}</h2><p>{seller && query.filter === "attention" && !query.search && query.range === "all" ? "Paid orders appear here when they are ready for delivery." : "Try another search, date range or status filter."}</p><button className="ob-text-link" onClick={() => changeQuery({ filter: "all", search: "", range: "all", from: "", to: "" })}>Clear filters</button></div>}
    {seller ? <SalesLedger rows={visible.rows} rates={rates} currency={currency} onSelect={selectRow}/> : selected.rows.length > 0 ? <>
      <div className="ob-order-ledger">{visible.rows.map(row => <article className={`ob-order-row ${row.attention ? "needs-action" : ""}${row.unreadMessages ? " has-unread" : ""}`} key={row.id}>
        <button type="button" className="ob-order-toggle" aria-label={`Order from ${row.subtitle}: ${row.title}`} onClick={() => selectRow(row.id)}>
          <span className="ob-order-name">
            <span className="ob-order-row-meta">{new Date(orderTime(row.created)).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })} · #{row.id.slice(0, 8)}{row.rail ? ` · ${row.rail}` : ""}</span>
            <strong>{row.title}</strong>
            <span className="ob-order-row-sub"><span>{row.subtitle}</span><OrderMessageBadge count={row.messageCount} unread={row.unreadMessages}/></span>
          </span>
          <span className={`ob-order-status ${row.attention ? "is-actionable" : ""}`}>{row.status}</span>
          <Money sats={row.amount} rates={rates} currency={currency}/>
          <span className="ob-order-row-open" aria-hidden="true">
            <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"><path d="m6 3 5 5-5 5"/></svg>
          </span>
        </button>
      </article>)}</div>
    </> : null}
    {selected.rows.length > size && <div className="ob-order-pagination-footer">{pagination(true)}</div>}
  </>;
}
