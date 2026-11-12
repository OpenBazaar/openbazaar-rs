import { test, expect, type Locator, type Page } from '@playwright/test'

// The test node holds only the built-in sample stores, and its directory does
// not list those. So `/directory` and `/follow` are answered here the way the
// node answers them (crates/node/src/directory.rs): a followed store has
// `added`, and an unfollowed store stays listed only while the directory names it.
type Held = { named: boolean; added: boolean }

async function marlow(page: Page) {
  const catalog = await (await page.request.get('/catalog')).json() as { stores: { key: string; name: string }[] }
  return catalog.stores.find(store => store.name === 'Marlow Goods')!.key
}
async function directory(page: Page, key: string, held: Held) {
  const calls: string[] = []
  const list = () => ({ stores: held.named || held.added ? [{ storeId: key, name: 'Marlow Goods', listings: 3, added: held.added }] : [] })
  await page.route('**/directory', route => route.fulfill({ json: list() }))
  await page.route('**/follow', async route => {
    calls.push(`${route.request().method()} ${route.request().postData()}`)
    held.added = route.request().method() === 'POST'
    await route.fulfill({ json: list() })
  })
  return calls
}
// `stay` goes back through the sidebar instead of loading the page again.
async function openStore(page: Page, stay = false) {
  if (stay) await page.getByRole('button', { name: 'Products', exact: true }).click()
  else await page.goto('/')
  await page.getByRole('button', { name: 'Open store Marlow Goods', exact: true }).first().click()
  await expect(page.getByRole('heading', { name: 'Marlow Goods', exact: true, level: 1 })).toBeVisible()
}
async function followedStores(page: Page) {
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'Followed stores', exact: true }).click()
  return page.locator('.ob-follow-list li').filter({ hasText: 'Marlow Goods' })
}
const follow = (page: Page) => page.getByRole('button', { name: 'Follow Marlow Goods', exact: true })
const following = (page: Page) => page.getByRole('button', { name: 'Following Marlow Goods. Unfollow', exact: true })

test('a store the directory names is followed and unfollowed from its page', async ({ page }) => {
  const key = await marlow(page)
  const calls = await directory(page, key, { named: true, added: false })
  await openStore(page)
  await expect(follow(page)).toHaveAttribute('aria-pressed', 'false')
  await follow(page).click()
  await expect(following(page)).toHaveAttribute('aria-pressed', 'true')
  await expect(following(page)).toHaveText('FollowingUnfollow')
  await expect(page.locator('.ob-toast')).toHaveText('Following Marlow Goods. Your node keeps it even if the directory drops it.')
  expect(calls).toEqual([`POST ${JSON.stringify({ storeId: key })}`])

  // The listing page's seller block reads the same state.
  await page.locator('.ob-storefront .ob-product-open').first().click()
  await expect(page.locator('.ob-detail-seller-row').getByRole('button', { name: 'Open store Marlow Goods', exact: true })).toBeVisible()
  await expect(page.locator('.ob-detail-seller-row').getByRole('button', { name: 'Following Marlow Goods. Unfollow', exact: true })).toBeVisible()

  await expect((await followedStores(page)).getByRole('button', { name: 'Remove', exact: true })).toBeVisible()

  await openStore(page, true)
  await following(page).click()
  await expect(follow(page)).toHaveAttribute('aria-pressed', 'false')
  await expect(page.locator('.ob-toast')).toHaveText('Stopped following Marlow Goods. The directory still names it.')
  expect(calls[1]).toBe(`DELETE ${JSON.stringify({ storeId: key })}`)
  const row = await followedStores(page)
  await expect(row).toBeVisible()
  await expect(row.getByRole('button', { name: 'Remove', exact: true })).toHaveCount(0)
})

test('a store followed by key can be unfollowed and followed again in the same place', async ({ page }) => {
  await directory(page, await marlow(page), { named: false, added: true })
  await openStore(page)
  await following(page).click()
  await expect(page.locator('.ob-toast')).toHaveText('Stopped following Marlow Goods. Your node no longer shows it.')
  await expect(follow(page)).toHaveAttribute('aria-pressed', 'false')
  await follow(page).click()
  await expect(following(page)).toHaveAttribute('aria-pressed', 'true')
})

test('the button is busy while the node answers and says why the node refused', async ({ page }) => {
  await directory(page, await marlow(page), { named: true, added: false })
  let answer = () => {}
  const asked = new Promise<void>(resolve => { answer = resolve })
  let refuse = true
  await page.route('**/follow', async route => {
    if (!refuse) return route.fallback()
    await asked
    await route.fulfill({ status: 500, json: { error: 'The store key could not be saved.' } })
  })
  await openStore(page)
  await follow(page).click()
  const busy = page.locator('.ob-follow button')
  await expect(busy).toBeDisabled()
  await expect(busy).toHaveText('Saving…')
  answer()
  await expect(page.locator('.ob-follow').getByRole('alert')).toHaveText('The store key could not be saved.')
  await expect(follow(page)).toBeEnabled()
  await expect(follow(page)).toHaveAttribute('aria-pressed', 'false')

  refuse = false
  await follow(page).click()
  await expect(following(page)).toBeVisible()
  await expect(page.locator('.ob-follow').getByRole('alert')).toHaveCount(0)
})

test('a sample store the directory does not list, and the own store, have no follow button', async ({ page }) => {
  const key = await marlow(page)
  const listed = page.waitForResponse('**/directory')
  await openStore(page)
  const stores = (await (await listed).json() as { stores: { storeId: string }[] }).stores
  expect(stores.map(store => store.storeId)).not.toContain(key)
  await page.waitForLoadState('networkidle')
  await expect(page.locator('.ob-follow')).toHaveCount(0)

  await directory(page, key, { named: true, added: false })
  await page.route('**/store/profile', async route => {
    const response = await route.fetch()
    await route.fulfill({ json: { ...await response.json(), storeKey: key } })
  })
  const profile = page.waitForResponse('**/store/profile')
  await openStore(page)
  await profile
  await page.waitForLoadState('networkidle')
  await expect(page.locator('.ob-follow')).toHaveCount(0)
})

// `/follows` is answered here the way the node answers it
// (crates/node/src/replicas.rs): the list comes back even when the store could
// not be fetched, with the reason on the row.
test('a store is followed by a pasted address, with progress and a plain reason when it fails', async ({ page }) => {
  const url = `http://${'a2'.repeat(28)}.onion`
  const posted: (string | null)[] = []
  let rows: object[] = []
  let selling = false
  let answer = () => {}
  const asked = new Promise<void>(resolve => { answer = resolve })
  await page.route('**/follows', async route => {
    const method = route.request().method()
    if (method === 'POST') {
      posted.push(route.request().postData())
      await asked
      rows = [selling ? { url, storeId: 'cd'.repeat(32), name: 'Cedar Workshop', listings: 2, fetched: 1, error: null } : { url, storeId: null, name: null, listings: 0, fetched: 0, error: 'that node is not selling' }]
    }
    if (method === 'DELETE') rows = []
    await route.fulfill({ json: { follows: rows, seeds: [] } })
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'Followed stores', exact: true }).click()
  await expect(page.getByLabel('Store key', { exact: true })).toBeVisible()
  const card = page.locator('.ob-follow-address')
  const field = card.getByLabel('Store address', { exact: true })
  const submit = card.locator('.ob-follow-form button')

  await field.fill('http://shop.example')
  await submit.click()
  await expect(card.getByRole('alert')).toHaveText("That is not a store address. Paste the address from the seller's storefront: http:// and a name that ends in .onion.")
  expect(posted).toEqual([])

  await field.fill(` ${url.slice('http://'.length)}/ `)
  await submit.click()
  await expect(submit).toBeDisabled()
  await expect(submit).toHaveText('Following…')
  await expect(card.getByRole('status')).toHaveText('Reaching the store. Over Tor this can take a minute.')
  answer()
  await expect(card.getByRole('alert')).toHaveText('That node answered, but it has no open store yet. Your node keeps the address and checks again.')
  expect(posted).toEqual([JSON.stringify({ url })])
  await expect(card.locator('.ob-follow-list li')).toContainText('Not reached yet')
  await expect(card.getByRole('status')).toHaveCount(0)

  selling = true
  await field.fill(url)
  await submit.click()
  await expect(page.locator('.ob-toast')).toHaveText('Following Cedar Workshop. Its listings are in Products.')
  await expect(card.locator('.ob-follow-list li')).toContainText('2 listings')
  await expect(card.getByRole('alert')).toHaveCount(0)
  await card.getByRole('button', { name: 'Remove', exact: true }).click()
  await expect(page.locator('.ob-toast')).toHaveText('Stopped following that store')
  await expect(card.locator('.ob-follow-list li')).toHaveCount(0)
})

for (const [width, dark] of [[320, false], [390, true]] as const) {
  test(`the follow button fits the store and listing pages at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 })
    await page.emulateMedia({ colorScheme: dark ? 'dark' : 'light' })
    await directory(page, await marlow(page), { named: true, added: true })
    await openStore(page)
    const fits = async (button: Locator) => {
      const box = (await button.boundingBox())!
      expect(box.height).toBeGreaterThanOrEqual(44)
      expect(box.x).toBeGreaterThanOrEqual(0)
      expect(box.x + box.width).toBeLessThanOrEqual(width)
      expect(await page.locator('.ob-content').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    }
    await fits(following(page))
    await page.locator('.ob-storefront .ob-product-open').first().click()
    await expect(page.locator('.ob-detail-seller-row')).toBeVisible()
    await fits(following(page))
    await following(page).click()
    await fits(follow(page))
  })
}
