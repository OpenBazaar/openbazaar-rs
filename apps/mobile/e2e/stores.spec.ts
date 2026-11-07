import { test, expect } from '@playwright/test'
import { checkStoreImage, parseStoreList, parseStorePage } from '../src/stores.ts'

// Storefront data from the real node, checked with the client's own parsers.
test('the node serves signed storefronts and hash-checked media to a paired client', async ({ page, request }) => {
  await page.goto('/')
  const token = await page.evaluate(() => (globalThis as { __OB_PAIR_TOKEN__?: string }).__OB_PAIR_TOKEN__)
  expect(token).toMatch(/^[0-9a-f]{64}$/)
  const headers = { 'X-Ob-Pair': token as string }

  expect((await request.get('/stores')).ok()).toBe(false)

  const list = parseStoreList(await (await request.get('/stores', { headers })).json())
  expect(list.rejected).toBe(0)
  for (const name of ['Marlow Goods', 'Hale Paper', 'North Kitchen'])
    expect(list.stores.map(s => s.name)).toContain(name)

  const marlow = list.stores.find(s => s.name === 'Marlow Goods')!
  const store = parseStorePage(await (await request.get(`/stores/${marlow.key}`, { headers })).json(), marlow.key)
  expect(store.store.key).toBe(marlow.key)
  expect(store.listings.length).toBeGreaterThan(0)
  for (const listing of store.listings) expect(listing.storeId).toBe(marlow.key)

  const avatar = store.store.avatar!
  const media = await request.get(`/media/${avatar}`, { headers })
  expect(media.ok()).toBe(true)
  const image = checkStoreImage(avatar, media.headers()['content-type'], new Uint8Array(await media.body()))
  expect(image).not.toBeNull()

  expect((await request.get(`/media/${'0'.repeat(64)}`, { headers })).status()).toBe(404)
  expect((await request.get(`/stores/${'0'.repeat(64)}`, { headers })).status()).toBe(404)
})
