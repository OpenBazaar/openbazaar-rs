import { test, expect, type Locator, type Page } from '@playwright/test'
import { mkdirSync } from 'node:fs'
import { parseStoreList } from '../src/stores.ts'
import { fakeStore } from './store-records.ts'

// `GET /stores` is the test node's own answer with the directory fields set
// the way a busy node would send them, plus a few more stores signed with
// throwaway keys. `/directory`, `/follow` and `/follows` are answered here as
// crates/node/src/directory.rs answers them. Store pages are the real node's.
const SHOTS = '/tmp/ob-store-directory-preview'
test.use({ deviceScaleFactor: 2 })
test.afterEach(({ page }) => page.unrouteAll({ behavior: 'ignoreErrors' }))

const LONG = 'Wanderlust Mountain Supply and Outfitters of the Northern Cascades'
type Row = { key: string; name: string; categories: string[]; listingCount: number; reviewCount: number; reviewAverage: number | null }
const REAL: Record<string, Omit<Row, 'key' | 'name'>> = {
  'Marlow Goods': { categories: ['Books', 'Home', 'Music'], listingCount: 12, reviewCount: 12, reviewAverage: 4.8 },
  'Hale Paper': { categories: ['Paper', 'Tools'], listingCount: 8, reviewCount: 3, reviewAverage: 4.3 },
  'North Kitchen': { categories: ['Grocery', 'Home'], listingCount: 6, reviewCount: 0, reviewAverage: null }
}

type Held = { followed: Set<string>; calls: string[]; keys: Record<string, string> }

async function fixture(page: Page): Promise<Held> {
  const held: Held = { followed: new Set(), calls: [], keys: {} }
  let rows: Record<string, unknown>[] = []
  const build = (body: { stores: Record<string, unknown>[] }) => {
    if (rows.length) return
    const real = parseStoreList(body).stores
    const art = real.find(store => store.name === 'Hale Paper')!
    const fake = (name: string, extra: Partial<Parameters<typeof fakeStore>[0]>, row: Omit<Row, 'key' | 'name'>, seed: number) => {
      const made = fakeStore({ name, region: '', tagline: '', seed, ...extra })
      return { key: made.key, name, record: made.record, own: false, version: 3, ...row }
    }
    const added = [
      fake('Lumen Coffee Roasters', { region: 'Portland, OR', tagline: 'Small-batch beans, roasted to order.', lightning: true, monero: true }, { categories: ['Grocery'], listingCount: 4, reviewCount: 2, reviewAverage: 5 }, 11),
      fake('Zephyr Audio', { region: 'Reykjavik', tagline: 'Hand-built tube amplifiers, turntables and speakers, tested for a week on the bench before they ship to a buyer anywhere in the world.', lightning: true, monero: true, zcash: true, avatar: art.avatar!, banner: art.banner! }, { categories: ['Audio', 'Electronics', 'Music', 'Tools'], listingCount: 21, reviewCount: 0, reviewAverage: null }, 12),
      fake(LONG, { region: 'WA' }, { categories: [], listingCount: 1, reviewCount: 0, reviewAverage: null }, 13)
    ]
    const enriched = body.stores.map(item => ({ ...item, ...(REAL[String(item.name)] ?? { categories: [], listingCount: 0, reviewCount: 0, reviewAverage: null }) }))
    rows = [...enriched, ...added]
    for (const item of rows) held.keys[String(item.name)] = String(item.key)
    held.followed.add(held.keys['Hale Paper'])
  }
  // `/directory` can be asked before `/stores`, so it reads the node's list itself.
  const load = async () => {
    if (rows.length) return
    const token = await page.evaluate(() => (globalThis as { __OB_PAIR_TOKEN__?: string }).__OB_PAIR_TOKEN__)
    build(await (await page.request.get('/stores', { headers: { 'X-Ob-Pair': token! } })).json())
  }
  await page.route('**/stores', async route => {
    if (route.request().method() !== 'GET') return route.continue()
    const response = await route.fetch()
    build(await response.json())
    await route.fulfill({ response, json: { stores: rows } })
  })
  const list = () => ({ stores: rows.filter(row => row.own !== true).map(row => ({ storeId: row.key, name: row.name, listings: row.listingCount, added: held.followed.has(String(row.key)), admitted: true })) })
  await page.route('**/directory', async route => { await load(); await route.fulfill({ json: list() }) })
  await page.route('**/follow', async route => {
    const storeId = JSON.parse(route.request().postData() ?? '{}').storeId as string
    held.calls.push(`${route.request().method()} ${storeId}`)
    if (route.request().method() === 'POST') held.followed.add(storeId)
    else held.followed.delete(storeId)
    await route.fulfill({ json: list() })
  })
  await page.route('**/follows', route => route.fulfill({ json: { follows: [] } }))
  return held
}

const sidebar = (page: Page) => page.getByRole('complementary', { name: 'Main navigation', exact: true })
const card = (page: Page, name: string) => page.locator('.ob-directory-card').filter({ has: page.getByRole('button', { name: `Open store ${name}`, exact: true }) })
const names = (page: Page) => page.locator('.ob-directory-title h2').allInnerTexts()
const segment = (page: Page, label: 'All stores' | 'Following') => page.locator('.ob-directory-segment').getByRole('button', { name: new RegExp(`^${label}`) })

async function openDirectory(page: Page) {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await sidebar(page).getByRole('button', { name: 'Stores', exact: true }).click()
  await expect(page.getByRole('heading', { name: /^Stores/, level: 1 })).toBeVisible()
  await expect(page.locator('.ob-directory-card').first()).toBeVisible()
}
async function theme(page: Page, dark: boolean) {
  const toggle = page.getByRole('switch', { name: 'Dark mode', exact: true })
  if (await toggle.isChecked() !== dark) await toggle.click()
}
async function noOverflow(page: Page) {
  const layout = await page.evaluate(() => {
    const content = document.querySelector('.ob-content')!
    const edge = Math.max(...[...document.querySelectorAll('.ob-directory-card, .ob-directory-controls > *')].map(el => el.getBoundingClientRect().right))
    return { scroll: content.scrollWidth, width: content.clientWidth, page: document.documentElement.scrollWidth, view: window.innerWidth, edge }
  })
  expect(layout.scroll).toBeLessThanOrEqual(layout.width + 1)
  expect(layout.page).toBeLessThanOrEqual(layout.view + 1)
  expect(layout.edge).toBeLessThanOrEqual(layout.view + 1)
}
async function settle(page: Page) {
  await expect.poll(() => page.locator('.ob-directory-banner-img').evaluateAll(images => images.length > 0 && images.every(image => image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0))).toBe(true)
  await expect.poll(() => page.locator('.ob-directory-avatar img').evaluateAll(images => images.every(image => image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0))).toBe(true)
}

test('Stores sits under Classifieds, is a permalink, and survives a reload', async ({ page }) => {
  await fixture(page)
  await openDirectory(page)
  const items = await page.getByRole('navigation', { name: 'Shopping', exact: true }).getByRole('button').evaluateAll(buttons => buttons.map(button => button.getAttribute('aria-label')))
  expect(items.slice(0, 4)).toEqual(['Products', 'Classifieds', 'Stores', 'Saved goods'])
  const stores = sidebar(page).getByRole('button', { name: 'Stores', exact: true })
  const classifieds = sidebar(page).getByRole('button', { name: 'Classifieds', exact: true })
  await expect(stores).toHaveAttribute('aria-current', 'page')
  const [a, b] = await Promise.all([classifieds, stores].map(async item => item.evaluate(el => ({ size: el.querySelector('svg')!.getAttribute('width'), stroke: getComputedStyle(el.querySelector('svg')!).strokeWidth, height: el.getBoundingClientRect().height, radius: getComputedStyle(el).borderRadius }))))
  expect(b).toEqual(a)
  await expect(page).toHaveURL(/#\/stores$/)
  await expect(page.locator('.ob-breadcrumb')).toHaveText('MarketplaceStores')
  await expect(page.locator('.ob-breadcrumb').getByRole('button', { name: 'Marketplace' })).toBeVisible()

  await page.reload()
  await expect(page.getByRole('heading', { name: /^Stores/, level: 1 })).toBeVisible()
  await expect(page.locator('.ob-directory-card')).toHaveCount(7)
  await expect(stores).toHaveAttribute('aria-current', 'page')

  await sidebar(page).getByRole('button', { name: 'Products', exact: true }).click()
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  await page.goBack()
  await expect(page.getByRole('heading', { name: /^Stores/, level: 1 })).toBeVisible()
  await page.goForward()
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
})

test('a store card shows the banner, avatar, tagline, region, listings, rating, rails and categories', async ({ page }) => {
  await fixture(page)
  await openDirectory(page)
  await settle(page)
  await expect(page.locator('.ob-directory-header h1 span')).toHaveText('7')
  await expect(page.getByRole('status').filter({ hasText: '7 stores' })).toBeVisible()

  const marlow = card(page, 'Marlow Goods')
  await expect(marlow.locator('img.ob-directory-banner-img')).toBeVisible()
  await expect(marlow.locator('.ob-directory-avatar img')).toBeVisible()
  expect((await marlow.locator('.ob-directory-banner').boundingBox())!.height).toBe(76)
  expect((await marlow.locator('.ob-directory-avatar').boundingBox())!.width).toBe(48)
  await expect(marlow.locator('.ob-directory-tagline')).not.toBeEmpty()
  await expect(marlow.locator('.ob-directory-meta')).toContainText('12 items')
  await expect(marlow.locator('.ob-directory-rating')).toHaveText('★ 4.8 (12)')
  await expect(marlow.locator('.ob-directory-rating')).toHaveAttribute('aria-label', 'Rated 4.8 out of 5 from 12 reviews')
  await expect(marlow.locator('.ob-directory-pill')).toHaveText(['BTC', 'Books', 'Home', 'Music'])

  const lumen = card(page, 'Lumen Coffee Roasters')
  await expect(lumen.locator('.ob-directory-banner img')).toHaveCount(0)
  await expect(lumen.locator('.ob-directory-avatar')).toHaveText('L')
  await expect(lumen.locator('.ob-directory-rating')).toHaveText('★ 5.0 (2)')
  await expect(lumen.locator('.ob-directory-pill')).toHaveText(['BTC', 'LN', 'XMR', 'Grocery'])
  expect((await lumen.locator('.ob-directory-banner').boundingBox())!.height).toBe(76)

  const zephyr = card(page, 'Zephyr Audio')
  await expect(zephyr.locator('.ob-directory-pill')).toHaveText(['BTC', 'LN', 'XMR', 'ZEC', 'Audio', 'Electronics', 'Music'])
  await expect(zephyr.locator('.ob-directory-rating')).toHaveCount(0)
  const clamp = await zephyr.locator('.ob-directory-tagline').evaluate(el => ({ lines: Math.round(el.getBoundingClientRect().height / parseFloat(getComputedStyle(el).lineHeight)), overflow: el.scrollHeight > el.clientHeight }))
  expect(clamp).toEqual({ lines: 2, overflow: true })

  await expect(card(page, 'My store').locator('.ob-directory-own')).toHaveText('Your store')
  await expect(card(page, 'My store').locator('.ob-directory-meta')).toContainText('No listings yet')
  await expect(card(page, 'My store').getByRole('button', { name: /^Follow/ })).toHaveCount(0)
  await expect(card(page, LONG).locator('.ob-directory-meta')).toContainText('1 item')
  await expect(page.locator('.ob-directory-meta').first()).toHaveCSS('font-variant-numeric', 'tabular-nums')
  await expect(page.locator('.ob-directory-header h1 span')).toHaveCSS('font-variant-numeric', 'tabular-nums')
})

test('search, Following, category, payment rail and sort narrow and order the stores', async ({ page }) => {
  const held = await fixture(page)
  await openDirectory(page)
  const search = page.getByRole('textbox', { name: 'Search stores', exact: true })
  await expect(search).toHaveAttribute('placeholder', 'Search stores by name, region, or category…')
  await expect(page.getByRole('button', { name: 'Clear filters', exact: true })).toHaveCount(0)
  expect(await names(page)).toEqual(['Zephyr Audio', 'Marlow Goods', 'Hale Paper', 'North Kitchen', 'Lumen Coffee Roasters', LONG, 'My store'])

  await search.fill('lumen')
  expect(await names(page)).toEqual(['Lumen Coffee Roasters'])
  await expect(page.locator('.ob-directory-header h1 span')).toHaveText('1')
  await expect(page.getByRole('status').filter({ hasText: '1 of 7 stores' })).toBeVisible()
  await search.fill('oregon')
  expect(await names(page)).toEqual(['Marlow Goods'])
  await search.fill('electronics')
  expect(await names(page)).toEqual(['Zephyr Audio'])
  await search.fill(held.keys['Lumen Coffee Roasters'].slice(0, 10))
  expect(await names(page)).toEqual(['Lumen Coffee Roasters'])
  await search.fill('nothing like this')
  await expect(page.getByRole('heading', { name: 'No stores match', exact: true })).toBeVisible()
  await page.getByRole('main').getByRole('button', { name: 'Clear filters', exact: true }).last().click()
  await expect(search).toHaveValue('')
  await expect(page.locator('.ob-directory-card')).toHaveCount(7)

  await expect(segment(page, 'All stores')).toHaveText('All stores 7')
  await expect(segment(page, 'Following')).toHaveText('Following 1')
  await segment(page, 'Following').click()
  expect(await names(page)).toEqual(['Hale Paper'])
  await expect(segment(page, 'Following')).toHaveAttribute('aria-pressed', 'true')
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click()
  await expect(segment(page, 'All stores')).toHaveAttribute('aria-pressed', 'true')

  const category = page.getByRole('combobox', { name: 'Category', exact: true })
  expect(await category.locator('option').allInnerTexts()).toEqual(['All categories', 'Audio (1)', 'Books (1)', 'Electronics (1)', 'Grocery (2)', 'Home (2)', 'Music (2)', 'Paper (1)', 'Tools (2)'])
  await category.selectOption('Home')
  expect(await names(page)).toEqual(['Marlow Goods', 'North Kitchen'])
  await expect(segment(page, 'All stores')).toHaveText('All stores 2')
  await category.selectOption('')

  const rail = page.getByRole('combobox', { name: 'Payment rail', exact: true })
  expect(await rail.locator('option').allInnerTexts()).toEqual(['All rails', 'Bitcoin', 'Lightning', 'Monero', 'Zcash'])
  await rail.selectOption({ label: 'Lightning' })
  expect(await names(page)).toEqual(['Zephyr Audio', 'Lumen Coffee Roasters'])
  await rail.selectOption({ label: 'Zcash' })
  expect(await names(page)).toEqual(['Zephyr Audio'])
  await rail.selectOption({ label: 'Bitcoin' })
  await expect(page.locator('.ob-directory-card')).toHaveCount(7)
  await category.selectOption('Tools')
  await rail.selectOption({ label: 'Monero' })
  expect(await names(page)).toEqual(['Zephyr Audio'])
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click()
  await expect(category).toHaveValue('')
  await expect(rail).toHaveValue('all')

  const sort = page.getByRole('combobox', { name: 'Sort stores', exact: true })
  expect(await sort.locator('option').allInnerTexts()).toEqual(['Most listings', 'Top rated', 'Name A–Z'])
  await sort.selectOption({ label: 'Top rated' })
  expect(await names(page)).toEqual(['Lumen Coffee Roasters', 'Marlow Goods', 'Hale Paper', 'My store', 'North Kitchen', LONG, 'Zephyr Audio'])
  await sort.selectOption({ label: 'Name A–Z' })
  expect(await names(page)).toEqual(['Hale Paper', 'Lumen Coffee Roasters', 'Marlow Goods', 'My store', 'North Kitchen', LONG, 'Zephyr Audio'])
  await expect(page.getByRole('button', { name: 'Clear filters', exact: true })).toHaveCount(0)
})

test('Follow works from a card without opening the store, and from the Follow a store dialog', async ({ page }) => {
  const held = await fixture(page)
  await openDirectory(page)
  const lumen = card(page, 'Lumen Coffee Roasters')
  await lumen.getByRole('button', { name: 'Follow Lumen Coffee Roasters', exact: true }).click()
  await expect(lumen.getByRole('button', { name: 'Following Lumen Coffee Roasters. Unfollow', exact: true })).toHaveAttribute('aria-pressed', 'true')
  await expect(page).toHaveURL(/#\/stores$/)
  await expect(page.locator('.ob-toast')).toContainText('Following Lumen Coffee Roasters')
  await expect(segment(page, 'Following')).toHaveText('Following 2')
  expect(held.calls).toEqual([`POST ${held.keys['Lumen Coffee Roasters']}`])

  await segment(page, 'Following').click()
  expect(await names(page)).toEqual(['Hale Paper', 'Lumen Coffee Roasters'])
  await card(page, 'Hale Paper').getByRole('button', { name: 'Following Hale Paper. Unfollow', exact: true }).click()
  await expect(segment(page, 'Following')).toHaveText('Following 1')
  expect(await names(page)).toEqual(['Lumen Coffee Roasters'])
  await card(page, 'Lumen Coffee Roasters').getByRole('button', { name: 'Following Lumen Coffee Roasters. Unfollow', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'You are not following any stores', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Browse all stores', exact: true }).click()
  await expect(page.locator('.ob-directory-card')).toHaveCount(7)

  await page.getByRole('button', { name: 'Follow a store', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Follow a store', exact: true })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole('textbox', { name: 'Store key or address', exact: true })).toBeFocused()
  await expect(dialog.getByRole('button', { name: 'Follow store', exact: true })).toBeDisabled()
  await dialog.getByRole('textbox').fill('not a key')
  await dialog.getByRole('button', { name: 'Follow store', exact: true }).click()
  await expect(dialog.getByRole('alert')).toContainText('That is not a store key or address')
  const key = held.keys['Zephyr Audio']
  await dialog.getByRole('textbox').fill(`  ${key.toUpperCase()} `)
  await dialog.getByRole('button', { name: 'Follow store', exact: true }).click()
  await expect(dialog).toBeHidden()
  await expect(page.locator('.ob-toast')).toHaveText('Store key saved. The node will keep its records.')
  await expect(segment(page, 'Following')).toHaveText('Following 1')
  await expect(card(page, 'Zephyr Audio').getByRole('button', { name: 'Following Zephyr Audio. Unfollow', exact: true })).toBeVisible()
  expect(held.calls.at(-1)).toBe(`POST ${key}`)

  await page.getByRole('button', { name: 'Follow a store', exact: true }).click()
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
})

test('an address is followed through the node, and a refused one says why', async ({ page }) => {
  await fixture(page)
  const onion = `http://${'a'.repeat(56)}.onion`
  const bodies: string[] = []
  await page.route('**/follows', async route => {
    if (route.request().method() === 'GET') return route.fulfill({ json: { follows: [] } })
    bodies.push(route.request().postData() ?? '')
    await route.fulfill({ json: { follows: [{ url: onion, storeId: 'ab'.repeat(32), name: 'Far Away Goods', listings: 2, fetched: 1, error: null }] } })
  })
  await openDirectory(page)
  await page.getByRole('button', { name: 'Follow a store', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Follow a store', exact: true })
  await dialog.getByRole('textbox').fill(onion)
  await dialog.getByRole('button', { name: 'Follow store', exact: true }).click()
  await expect(dialog).toBeHidden()
  expect(bodies).toEqual([JSON.stringify({ url: onion })])
  await expect(page.locator('.ob-toast')).toContainText('Following Far Away Goods')
})

test('a card opens the store, and Back returns to the directory', async ({ page }) => {
  const held = await fixture(page)
  await openDirectory(page)
  const key = held.keys['Marlow Goods']
  // Anywhere on the card, not just the name: the name's link stretches over it.
  await card(page, 'Marlow Goods').click({ position: { x: 150, y: 130 } })
  await expect(page.getByRole('heading', { name: 'Marlow Goods', exact: true, level: 1 })).toBeVisible()
  await expect(page).toHaveURL(new RegExp(`#/store/${key}$`))
  await expect(page.locator('.ob-breadcrumb')).toContainText('Stores')
  await page.getByRole('button', { name: 'Back to stores', exact: true }).click()
  await expect(page).toHaveURL(/#\/stores$/)
  await expect(page.locator('.ob-directory-card')).toHaveCount(7)

  await card(page, 'Marlow Goods').getByRole('button', { name: 'Open store Marlow Goods', exact: true }).focus()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('heading', { name: 'Marlow Goods', exact: true, level: 1 })).toBeVisible()
  await page.goBack()
  await expect(page).toHaveURL(/#\/stores$/)
  await expect(page.getByRole('heading', { name: /^Stores/, level: 1 })).toBeVisible()
  await page.locator('.ob-breadcrumb').getByRole('button', { name: 'Marketplace' }).click()
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
})

test('an empty node says so and offers to follow a store', async ({ page }) => {
  await fixture(page)
  await page.route('**/stores', route => route.request().method() === 'GET' ? route.fulfill({ json: { stores: [] } }) : route.continue())
  await openDirectoryEmpty(page)
  await expect(page.getByRole('heading', { name: 'No stores yet', exact: true })).toBeVisible()
  await page.getByRole('main').getByRole('button', { name: 'Follow a store', exact: true }).last().click()
  await expect(page.getByRole('dialog', { name: 'Follow a store', exact: true })).toBeVisible()
})
async function openDirectoryEmpty(page: Page) {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  await sidebar(page).getByRole('button', { name: 'Stores', exact: true }).click()
}

test('the directory fits wide, tablet and 320px screens in light and dark, with previews', async ({ page }) => {
  mkdirSync(SHOTS, { recursive: true })
  await fixture(page)
  const grid = (locator: Locator) => locator.evaluate(el => getComputedStyle(el).gridTemplateColumns.split(' ').length)

  await page.setViewportSize({ width: 1366, height: 1200 })
  await openDirectory(page)
  await settle(page)
  expect(await grid(page.locator('.ob-directory-grid'))).toBe(3)
  await noOverflow(page)
  await page.screenshot({ path: `${SHOTS}/01-stores-light.png`, animations: 'disabled' })

  await theme(page, true)
  await expect(page.locator('html')).toHaveAttribute('data-ob-theme', 'dark')
  await noOverflow(page)
  await page.screenshot({ path: `${SHOTS}/02-stores-dark.png`, animations: 'disabled' })

  await page.setViewportSize({ width: 820, height: 1024 })
  expect(await grid(page.locator('.ob-directory-grid'))).toBe(2)
  await noOverflow(page)

  await page.setViewportSize({ width: 320, height: 1500 })
  expect(await grid(page.locator('.ob-directory-grid'))).toBe(1)
  await noOverflow(page)
  await page.screenshot({ path: `${SHOTS}/04-stores-mobile-320-dark.png`, animations: 'disabled' })

  await theme(page, false)
  await expect(page.locator('html')).toHaveAttribute('data-ob-theme', 'light')
  await noOverflow(page)
  await page.screenshot({ path: `${SHOTS}/03-stores-mobile-320.png`, animations: 'disabled' })
  await page.getByRole('button', { name: 'Follow a store', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Follow a store', exact: true })).toBeVisible()
  await noOverflow(page)
  const box = await page.getByRole('dialog', { name: 'Follow a store', exact: true }).boundingBox()
  expect(box!.x).toBeGreaterThanOrEqual(0)
  expect(box!.x + box!.width).toBeLessThanOrEqual(320)
})
