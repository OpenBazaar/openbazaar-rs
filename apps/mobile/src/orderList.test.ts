import assert from 'node:assert/strict'
import { test } from 'node:test'
import { indexOrders, selectOrders, orderPage, sameOrderSnapshot, salesRefreshInterval, type OrderListRow, type OrderQuery } from './orderList.ts'

const now = new Date(2026, 10, 12, 12).getTime()
const rows: OrderListRow[] = Array.from({ length: 1000 }, (_, i) => ({ id: String(i).padStart(8, '0'), title: `Product ${i}`, subtitle: 'Store', status: i % 4 === 0 ? 'Paid' : 'Completed', amount: 1000 + i, created: (now - i * 86400000) / 1000, attention: i % 4 === 0, closed: i % 4 !== 0, hint: '' }))
const query: OrderQuery = { filter: 'all', search: '', sort: 'newest', range: 'all', from: '', to: '' }
const select = (change: Partial<OrderQuery> = {}) => selectOrders(indexOrders(rows), { ...query, ...change }, now)

test('1,000 orders produce bounded, complete pages without duplication', () => {
  const selected = select()
  assert.deepEqual(selected.counts, { all: 1000, attention: 250, progress: 0, closed: 750 })
  const ids = []
  for (let page = 0; page < 40; page++) {
    const result = orderPage(selected.rows, page, 25, null)
    assert.equal(result.rows.length, 25)
    assert.equal(result.pages, 40)
    ids.push(...result.rows.map(row => row.id))
  }
  assert.equal(new Set(ids).size, 1000)
  assert.equal(orderPage(selected.rows, 99, 50, null).page, 19)
  assert.equal(orderPage([], 99, 25, null).page, 0)
})

test('search finds an order outside the first page and updates scoped counts', () => {
  assert.equal(select({ search: 'Product 999' }).rows[0].id, '00000999')
  assert.equal(select({ search: '00000999' }).counts.all, 1)
  assert.equal(select({ search: 'missing' }).rows.length, 0)
  assert.equal(select({ filter: 'attention' }).rows.length, 250)
})

test('priority puts the oldest actionable orders first; other sorts use date or total', () => {
  assert.equal(select({ sort: 'priority' }).rows[0].id, '00000996')
  assert.equal(select({ sort: 'oldest' }).rows[0].id, '00000999')
  assert.equal(select({ sort: 'highest' }).rows[0].id, '00000999')
  assert.equal(select({ sort: 'lowest' }).rows[0].id, '00000000')
  const tied = indexOrders([{ ...rows[0], id: 'b' }, { ...rows[0], id: 'a' }])
  assert.deepEqual(selectOrders(tied, query, now).rows.map(row => row.id), ['a', 'b'])
})

test('relative and custom dates include the full final calendar day', () => {
  assert.equal(select({ range: '7' }).rows.length, 8)
  assert.equal(select({ range: '30' }).rows.length, 31)
  assert.equal(select({ range: 'custom', from: '2026-11-12', to: '2026-11-12' }).rows.length, 1)
  assert.equal(select({ range: 'custom', from: '2026-11-12', to: '2026-11-01' }).rows.length, 0)
  const day = new Date(2026, 10, 1, 23, 59, 59).getTime()
  const special = indexOrders([{ ...rows[0], created: day }, { ...rows[1], created: day + 1000 }])
  assert.equal(selectOrders(special, { ...query, range: 'custom', from: '2026-11-01', to: '2026-11-01' }, now).rows.length, 1)
})

test('incoming orders keep an expanded detail mounted across page boundaries', () => {
  const original = select().rows
  const expanded = original[24].id
  const inserted = [{ ...original[0], id: 'incoming' }, ...original]
  const page = orderPage(inserted, 0, 25, expanded)
  assert.equal(page.page, 1)
  assert.ok(page.rows.some(row => row.id === expanded))
  assert.equal(orderPage(original.slice(0, 10), 39, 25, null).page, 0)
})

test('unchanged snapshots stay stable but delivery and payment changes are detected', () => {
  assert.ok(sameOrderSnapshot(rows, structuredClone(rows)))
  assert.equal(sameOrderSnapshot(rows, [{ ...rows[0], status: 'Shipped' }, ...rows.slice(1)]), false)
  assert.equal(salesRefreshInterval([{ status: 'Paid' }], true), 10000)
  assert.equal(salesRefreshInterval([{ status: 'Completed' }], true), 60000)
  assert.equal(salesRefreshInterval([{ status: 'Paid' }], false), 60000)
})
