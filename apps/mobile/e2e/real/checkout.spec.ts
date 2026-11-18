import { test, expect } from './harness.ts'

// A buyer node follows a seller node on this machine, orders, and both see the order.
test('a buyer orders from a followed store and both nodes show the order', async ({ page, seller, buyer }) => {
  await seller.addListing({ title: 'Field notes zine', kind: 'digital', price: 15000 })
  await buyer.follow(seller)
  await page.goto(buyer.ui)
  await page.getByLabel('Search stores and listings', { exact: true }).fill('Field notes zine')
  await page.getByRole('button', { name: 'View Field notes zine', exact: true }).click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Send to this test node.')
  const created = page.waitForResponse(r => new URL(r.url()).pathname === '/checkout' && r.request().method() === 'POST')
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  expect((await created).ok()).toBe(true)
  await expect(page.getByRole('heading', { name: 'Order created', exact: true })).toBeVisible()
  const payment = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  // The seller's node answers the order over loopback; then the buyer can pay.
  await expect(payment).toContainText('Awaiting payment', { timeout: 30000 })

  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Your orders' }).click()
  await expect(page.getByText('Field notes zine').first()).toBeVisible()

  await expect.poll(async () => (await seller.api('GET', '/sales')).sales.length, { timeout: 20000 }).toBe(1)
  const sellerPage = await page.context().newPage()
  await sellerPage.goto(seller.ui)
  await sellerPage.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Sales' }).click()
  await expect(sellerPage.getByText('Field notes zine').first()).toBeVisible()
})
