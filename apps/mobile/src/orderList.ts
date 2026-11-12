export type OrderListRow = {
  id: string; title: string; subtitle: string; status: string; amount: number;
  created: number; attention: boolean; closed: boolean; hint: string;
}
export type OrderFilter = 'all' | 'attention' | 'progress' | 'closed'
export type OrderSort = 'priority' | 'newest' | 'oldest' | 'highest' | 'lowest'
export type OrderRange = 'all' | '7' | '30' | '90' | 'custom'
export type OrderQuery = { filter: OrderFilter; search: string; sort: OrderSort; range: OrderRange; from: string; to: string }

export const orderTime = (created: number) => created < 1e12 ? created * 1000 : created
export function indexOrders(rows: OrderListRow[]) {
  return rows.map(row => ({ row, time: orderTime(row.created), search: `${row.title} ${row.subtitle} ${row.id}`.toLocaleLowerCase() }))
}

// Calendar boundaries keep the end date inclusive, including across DST changes.
function calendarDate(value: string, followingDay = false): number {
  const [year, month, day] = value.split('-').map(Number)
  return new Date(year, month - 1, day + (followingDay ? 1 : 0)).getTime()
}

export function selectOrders(index: ReturnType<typeof indexOrders>, query: OrderQuery, now = Date.now()) {
  const search = query.search.trim().toLocaleLowerCase()
  const from = query.range === 'custom' ? (query.from ? calendarDate(query.from) : -Infinity) : query.range === 'all' ? -Infinity : now - Number(query.range) * 86400000
  const to = query.range === 'custom' && query.to ? calendarDate(query.to, true) : Infinity
  const counts = { all: 0, attention: 0, progress: 0, closed: 0 }
  const matching = index.filter(item => {
    if (item.time < from || item.time >= to || !item.search.includes(search)) return false
    counts.all++
    if (item.row.attention) counts.attention++
    if (item.row.closed) counts.closed++
    if (!item.row.closed && !item.row.attention) counts.progress++
    return query.filter === 'all' || (query.filter === 'attention' ? item.row.attention : query.filter === 'closed' ? item.row.closed : !item.row.closed && !item.row.attention)
  })
  matching.sort((a, b) => {
    let difference = 0
    if (query.sort === 'priority') difference = Number(b.row.attention) - Number(a.row.attention) || (a.row.attention ? a.time - b.time : b.time - a.time)
    if (query.sort === 'newest') difference = b.time - a.time
    if (query.sort === 'oldest') difference = a.time - b.time
    if (query.sort === 'highest') difference = b.row.amount - a.row.amount
    if (query.sort === 'lowest') difference = a.row.amount - b.row.amount
    return difference || a.row.id.localeCompare(b.row.id)
  })
  return { rows: matching.map(item => item.row), counts }
}

/** Keep an open order mounted if an incoming order moves its page boundary. */
export function orderPage(rows: OrderListRow[], requestedPage: number, size: number, expanded: string | null) {
  const pages = Math.max(1, Math.ceil(rows.length / size))
  const openIndex = expanded ? rows.findIndex(row => row.id === expanded) : -1
  const page = openIndex >= 0 ? Math.floor(openIndex / size) : Math.max(0, Math.min(requestedPage, pages - 1))
  const start = page * size
  return { rows: rows.slice(start, start + size), page, pages, start }
}

/** Preserve React state and DOM when a poll returns the same parsed collection. */
export function sameOrderSnapshot<T>(previous: T, next: T): boolean {
  return JSON.stringify(previous) === JSON.stringify(next)
}

export function salesRefreshInterval(sales: { status: string }[] | null, viewingSales: boolean): number {
  return viewingSales && (!sales || sales.some(sale => !['Completed', 'Declined by the seller'].includes(sale.status))) ? 10000 : 60000
}
