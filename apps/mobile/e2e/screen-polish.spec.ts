import { test, expect, type Page, type TestInfo } from '@playwright/test'

async function navigation(page: Page, label: string, group?: string) {
  const drawer = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await drawer.isVisible()) await drawer.click()
  const scope = group ? page.getByRole('navigation', { name: group, exact: true }) : page
  await scope.getByRole('button', { name: label, exact: true }).click()
}
async function setting(page: Page, label: string) {
  await navigation(page, 'Settings')
  await page.getByRole('navigation', { name: 'Settings', exact: true }).getByRole('button', { name: label, exact: true }).click()
}
async function capture(page: Page, info: TestInfo, name: string) {
  const content = page.locator('.ob-content')
  expect(await content.evaluate(el => el.scrollWidth <= el.clientWidth + 1), `${name} should not scroll horizontally`).toBeTruthy()
  await page.screenshot({ path: info.outputPath(`${name}.png`) })
}

for (const [width, dark] of [[320, false], [768, true], [1280, false], [1920, true]] as const) {
  test(`seller forms, variants, sales and settings fit ${width}px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 })
    await page.emulateMedia({ colorScheme: dark ? 'dark' : 'light' })
    // Read-only presentation fixture; mutations are still covered by seller.spec.ts.
    await page.route('**/store/profile', async route => {
      const response = await route.fetch()
      await route.fulfill({ json: { ...await response.json(), role: 'seller', setUp: true, name: 'Brook and Pine', tagline: 'Considered goods for everyday living' } })
    })
    await page.route('**/coupons', route => route.fulfill({ json: { coupons: [{ code: 'WELCOME10', percentBps: 1000, currency: 'USD', startsOn: 20260101, endsOn: 20261231 }] } }))
    await page.route('**/offers', route => route.fulfill({ json: { offers: [{ id: 'ef'.repeat(16), listingId: 'c'.repeat(32), variantId: '0'.repeat(16), quantity: 2, amount: 4500, currency: 'USD' }] } }))
    await page.route('**/sales', route => route.fulfill({ json: { sales: [{
      id: 'ab'.repeat(16), created: 1794000000, status: 'Paid', network: 'testnet4',
      lines: [{ title: 'Hand-thrown stoneware mug', listingHash: 'c'.repeat(64), quantity: 2, kind: 'physical', priceSats: 25000 }],
      itemsSats: 50000, shippingSats: 3000, totalSats: 53000, receivedSats: 53000, confirmations: 1,
      shipTo: 'Test Buyer\n1 Test Street\nTest City, 00000', txids: ['d'.repeat(64)]
    }] } }))
    await page.goto('/')
    await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
    await navigation(page, 'Storefront', 'Your store')
    await expect(page.getByRole('navigation', { name: 'Shopping categories', exact: true })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Brook and Pine', exact: true, level: 1 })).toBeVisible()
    await capture(page, info, 'storefront')

    await setting(page, 'Store settings')
    const name = page.getByLabel('Display name', { exact: true })
    await expect(name).toBeVisible()
    const grid = name.locator('..').locator('..')
    const nameBox = await name.boundingBox()
    const gridBox = await grid.boundingBox()
    expect(nameBox!.width).toBeGreaterThan(gridBox!.width * .9)
    await capture(page, info, 'store-settings')

    await navigation(page, 'Listings', 'Your store')
    await page.getByRole('button', { name: 'New listing', exact: true }).click()
    await page.getByRole('button', { name: 'Configure fulfillment', exact: true }).click()
    await page.getByRole('button', { name: 'Add option', exact: true }).click()
    await page.getByLabel('Option name', { exact: true }).fill('Size')
    await page.getByLabel('Values, separated by commas', { exact: true }).fill('Small, Medium, Large')
    const variant = page.locator('.ob-variant-row').first()
    await expect(variant).toBeVisible()
    await variant.scrollIntoViewIfNeeded()
    for (const control of await variant.locator('input:not([type="checkbox"]), select').all()) {
      const metrics = await control.evaluate(el => ({ font: parseFloat(getComputedStyle(el).fontSize), height: el.getBoundingClientRect().height }))
      expect(metrics.font).toBeGreaterThanOrEqual(width >= 960 ? 14 : 16)
      expect(metrics.height).toBeGreaterThanOrEqual(width >= 960 ? 36 : 44)
    }
    if (width < 1280) await expect(variant.getByText('SKU', { exact: true })).toBeVisible()
    await capture(page, info, 'variants')
    await page.getByRole('button', { name: 'Add a shipping service', exact: true }).click()
    await page.getByLabel('Service', { exact: true }).last().scrollIntoViewIfNeeded()
    await capture(page, info, 'shipping-services')

    await navigation(page, 'Coupons', 'Your store')
    const coupon = page.locator('.ob-coupon-list tbody tr')
    await expect(coupon).toContainText('WELCOME10')
    expect((await coupon.locator('td').first().boundingBox())!.width).toBeGreaterThan(width <= 640 ? 180 : 90)
    await capture(page, info, 'coupons')

    await navigation(page, 'Sales', 'Your store')
    await page.getByRole('navigation', { name: 'Sales views', exact: true }).getByRole('button', { name: 'Offers', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Offers', exact: true })).toBeVisible()
    await capture(page, info, 'offers')
    await page.getByRole('navigation', { name: 'Sales views', exact: true }).getByRole('button', { name: 'Orders', exact: true }).click()
    await page.getByRole('button', { name: 'Sale Hand-thrown stoneware mug', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Ship this order', exact: true })).toBeVisible()
    await capture(page, info, 'sale-details')

    for (const label of ['Content filters', 'Addresses', 'Store labels', 'Home node', 'Lightning', 'Followed stores']) {
      await setting(page, label)
      await expect(page.getByRole('heading', { name: label === 'Addresses' ? 'Your addresses' : label, exact: true, level: 1 })).toBeVisible()
      await capture(page, info, label.toLowerCase().replaceAll(' ', '-'))
    }
  })
}

const STORE_KEY = 'ab'.repeat(32)
function saleRow(id: string, status: string, created: number) {
  return {
    id, created, status, network: 'testnet4',
    lines: [{ title: 'Hand-thrown stoneware mug', listingHash: 'c'.repeat(64), quantity: 1, kind: 'physical', priceSats: 25000 }],
    itemsSats: 25000, shippingSats: 3000, totalSats: 28000, receivedSats: status === 'Paid' ? 28000 : 0, confirmations: status === 'Paid' ? 1 : 0,
    shipTo: 'Test Buyer\n1 Test Street\nTest City, 00000', txids: status === 'Paid' ? ['d'.repeat(64)] : []
  }
}

test('the Sales sidebar badge shows new sales, then the total after Sales is opened', async ({ page }) => {
  const seen = saleRow('aa'.repeat(16), 'Awaiting payment', 1794000000)
  const shipped = saleRow('bb'.repeat(16), 'Shipped', 1794000001)
  const fresh = saleRow('cc'.repeat(16), 'Paid', 1796000000)
  let sales = [seen, shipped, fresh]
  await page.addInitScript(({ key, openedAt, statuses }) => {
    localStorage.setItem(key, JSON.stringify({ openedAt, statuses }))
  }, { key: `ob-sales-seen:${STORE_KEY}`, openedAt: 1795000000, statuses: { [seen.id]: seen.status, [shipped.id]: shipped.status } })
  await page.route('**/store/profile', async route => {
    const response = await route.fetch()
    await route.fulfill({ json: { ...await response.json(), role: 'seller', setUp: true, name: 'Brook and Pine', storeKey: STORE_KEY } })
  })
  await page.route('**/sales', route => route.fulfill({ json: { sales } }))
  await page.route('**/offers', route => route.fulfill({ json: { offers: [] } }))
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  const badge = page.getByRole('navigation', { name: 'Your store', exact: true }).locator('.ob-nav-count')
  await expect(badge).toHaveText('1')
  await expect(badge).toHaveClass(/is-new/)
  await expect(badge).toHaveAttribute('aria-label', '3 sales, 1 new')
  await page.locator('.ob-sidebar').screenshot({ path: '/tmp/sales-badge-new.png' })

  await navigation(page, 'Sales', 'Your store')
  await expect(page.getByRole('heading', { name: 'Sales', exact: true })).toBeVisible()
  await navigation(page, 'Products')
  await expect(badge).toHaveText('3')
  await expect(badge).not.toHaveClass(/is-new/)
  await expect(badge).toHaveAttribute('aria-label', '3 sales')
  await page.locator('.ob-sidebar').screenshot({ path: '/tmp/sales-badge-neutral.png' })

  sales = [seen, shipped, fresh, saleRow('dd'.repeat(16), 'Awaiting payment', 1797000000)]
  await expect(badge).toHaveClass(/is-new/, { timeout: 15000 })
  await expect(badge).toHaveText('1')
  await expect(badge).toHaveAttribute('aria-label', '4 sales, 1 new')
})

