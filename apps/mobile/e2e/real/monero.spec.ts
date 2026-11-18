import { test, expect } from './harness.ts'

// Monero direct pay on an offline stagenet monerod (genesis only, so nothing is
// paid): the seller takes Monero, the buyer orders with it, and the order shows
// the seller's stagenet subaddress, the XMR amount from the fixed test price,
// and a Monero QR. Skips without monerod.
test('a Monero order shows the seller’s stagenet subaddress and the XMR amount', async ({ page, monero }) => {
  const { seller, buyer } = monero
  await page.goto(seller.ui)
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Monero', exact: true }).click()
  await page.getByRole('switch', { name: /Your store/ }).click()
  await expect.poll(async () => (await seller.api('GET', '/store/profile')).acceptsXmr).toBe(true)
  await seller.addListing({ title: 'Field notes zine', kind: 'digital', price: 15000 })
  await buyer.follow(seller)

  await page.goto(buyer.ui)
  await page.getByLabel('Search stores and listings', { exact: true }).fill('Field notes zine')
  await page.getByRole('button', { name: 'View Field notes zine', exact: true }).click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Send to this test node.')
  await page.getByRole('radio', { name: /^Monero/ }).check()
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  await expect.poll(async () => (await buyer.api('GET', '/orders')).orders[0]?.payment?.rail, { timeout: 30000 }).toBe('xmr')
  const { payment } = (await buyer.api('GET', '/orders')).orders[0]
  expect(payment).toMatchObject({ network: 'stagenet', amountXmr: '0.05', satsPerXmr: 300000 })

  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Your orders' }).click()
  await page.getByRole('button', { name: /^Order from Brook and Pine/ }).first().click()
  const region = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  await expect(region).toContainText('Awaiting payment')
  await expect(region.locator('code').filter({ hasText: payment.subaddress })).toBeVisible()
  await expect(region.locator('code').filter({ hasText: /^0\.05$/ })).toBeVisible()
  await expect(region.getByRole('img', { name: 'Monero payment QR code' })).toBeVisible()
})
