import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { CurrencyCode, RateTable } from "../catalog";
import { indexOrders, orderPage, selectOrders, type OrderFilter, type OrderListRow, type OrderQuery, type OrderSort, type OrderRange } from "../orderList";
import { Money } from "./Marketplace";
import { SalesLedger } from "./SalesLedger";

type Props = { rows: OrderListRow[]; rates: RateTable; currency: CurrencyCode; renderDetail: (id: string) => ReactNode; seller?: boolean };
export function OrderWorkspace({ rows, rates, currency, renderDetail, seller = false }: Props) {
  const [query, setQuery] = useState<OrderQuery>(() => ({ filter: seller && rows.some(row => row.attention) ? "attention" : "all", search: "", sort: "priority", range: "all", from: "", to: "" }));
  const [page, setPage] = useState(0);
  const [size, setSize] = useState(25);
  const [expanded, setExpanded] = useState<string | null>(null);
  const top = useRef<HTMLDivElement>(null);
  const queue = useRef<HTMLDivElement>(null);
  const indexed = useMemo(() => indexOrders(rows), [rows]);
  const selected = useMemo(() => selectOrders(indexed, query), [indexed, query]);
  const visible = orderPage(selected.rows, page, size, expanded);
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
    setExpanded(null);
    startOfList();
  }
  function changePage(next: number, fromBottom = false) {
    setExpanded(null);
    setPage(next);
    startOfList();
    if (fromBottom) {
      if (!seller) top.current?.scrollIntoView({ block: "start" });
      top.current?.focus({ preventScroll: true });
    }
  }
  function closeOrder() {
    setPage(visible.page);
    setExpanded(null);
    requestAnimationFrame(() => { if (document.activeElement === document.body) top.current?.focus({ preventScroll: true }); });
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
      <label>Per page<select aria-label="Orders per page" value={size} onChange={event => { setSize(Number(event.target.value)); setPage(0); setExpanded(null); startOfList(); }}><option value={25}>25</option><option value={50}>50</option></select></label>
      {query.range === "custom" && <><label>From<input type="date" aria-label="From date" value={query.from} onChange={event => changeQuery({ from: event.target.value })}/></label><label>Through<input type="date" aria-label="Through date" min={query.from || undefined} value={query.to} onChange={event => changeQuery({ to: event.target.value })}/></label></>}
      {(query.search || query.range !== "all") && <button className="ob-text-link" onClick={() => changeQuery({ search: "", range: "all", from: "", to: "" })}>Reset search & dates</button>}
    </div>
    {invalidDates && <p className="ob-form-error" role="alert">The end date must be on or after the start date.</p>}
    <div className="ob-order-list-meta">
      <p role="status" aria-live="polite">{selected.rows.length ? `${(visible.start + 1).toLocaleString()}–${Math.min(visible.start + size, selected.rows.length).toLocaleString()} of ${selected.rows.length.toLocaleString()} ${countNoun}` : `0 ${noun}`}</p>
      {selected.rows.length > size && pagination()}
    </div>
    {!selected.rows.length && <div className="ob-order-no-results"><h2>{seller && query.filter === "attention" && !query.search && query.range === "all" ? "No orders to fulfill" : `No matching ${noun}`}</h2><p>{seller && query.filter === "attention" && !query.search && query.range === "all" ? "Paid orders appear here when they are ready for delivery." : "Try another search, date range or status filter."}</p><button className="ob-text-link" onClick={() => changeQuery({ filter: "all", search: "", range: "all", from: "", to: "" })}>Clear filters</button></div>}
    {seller ? <SalesLedger rows={visible.rows} allRows={rows} selected={expanded} rates={rates} currency={currency} onSelect={id => { setPage(visible.page); setExpanded(id); }} onClose={closeOrder} renderDetail={renderDetail}/> : selected.rows.length > 0 ? <>
      <div className="ob-order-ledger">{visible.rows.map(row => <article className={`ob-order-row ${row.attention ? "needs-action" : ""}`} key={row.id}>
        <button className="ob-order-toggle" aria-label={seller ? `Sale ${row.title}` : `Order from ${row.subtitle}`} aria-expanded={expanded === row.id} onClick={() => { setPage(visible.page); setExpanded(expanded === row.id ? null : row.id); }}>
          <span className="ob-order-name"><span className="ob-order-row-meta">{new Date(row.created * (row.created < 1e12 ? 1000 : 1)).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })} · #{row.id.slice(0,8)}</span><strong>{row.title}</strong><span>{seller ? row.hint : row.subtitle}</span></span>
          <span className={`ob-order-status ${row.attention ? "is-actionable" : ""}`}>{row.status}</span><Money sats={row.amount} rates={rates} currency={currency}/><span className="ob-order-row-open" aria-hidden="true">{expanded === row.id ? "−" : "+"}</span>
        </button>
        {expanded === row.id && renderDetail(row.id)}
      </article>)}</div>
    </> : null}
    {selected.rows.length > size && <div className="ob-order-pagination-footer">{pagination(true)}</div>}
  </>;
}
