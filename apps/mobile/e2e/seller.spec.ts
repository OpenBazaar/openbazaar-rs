import { test, expect, type Page } from '@playwright/test'

// Two real disposable nodes (see server.mjs): a fresh seller node and the
// shop's buyer node. The buyer follows the seller's node, orders, and the
// seller's node answers with payment terms over its order mail.
const SELLER_UI = `http://127.0.0.1:${process.env.OB_E2E_SELLER_UI_PORT || 48174}`
const SELLER_NODE = `http://127.0.0.1:${process.env.OB_E2E_SELLER_NODE_PORT || 48188}`
const PASSPHRASE = 'seller e2e passphrase'
// A valid 1×1 PNG; the node only keeps real images.
const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64'
)
const image = (name: string) => ({ name, mimeType: 'image/png', buffer: PNG })

test.describe.configure({ mode: 'serial' })

test.afterEach(async ({ page }) => {
  await page.unrouteAll({ behavior: 'wait' })
})

async function protectWallet(page: Page) {
  await expect(page.getByRole('heading', { name: 'Protect this wallet' })).toBeVisible()
  await page.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
  await page.getByLabel('Repeat passphrase', { exact: true }).fill(PASSPHRASE)
  await page.getByRole('button', { name: 'Set passphrase' }).click()
  await expect(page.getByRole('heading', { name: 'Write down your backup phrase' })).toBeVisible()
  const words = await page.locator('.ob-backup-phrase li').allTextContents()
  await page.getByRole('button', { name: 'I wrote it down' }).click()
  const confirm = page.getByRole('heading', { name: 'Confirm your backup' })
  if (await confirm.isVisible()) {
    for (const label of await page.locator('.ob-wallet-lock .ob-field span').allTextContents()) {
      const position = Number(label.replace('Word ', ''))
      await page.getByLabel(label, { exact: true }).fill(words[position - 1])
    }
    await page.getByRole('button', { name: 'Confirm backup' }).click()
  }
}

test('a new seller protects the wallet, chooses to sell, and opens a store', async ({ page }) => {
  await expect.poll(async () => (await page.request.get(`${SELLER_UI}/network`)).status(), { timeout: 20000 }).toBe(200)
  await page.goto(SELLER_UI)
  await expect(page.getByText('Step 1 of 2')).toBeVisible()
  await protectWallet(page)
  await expect(page.getByRole('heading', { name: 'How will you use OpenBazaar?' })).toBeVisible()
  await page.getByRole('button', { name: /^Sell/ }).click()

  await expect(page.getByRole('heading', { name: 'Set up your store' })).toBeVisible()
  await page.getByLabel('Store name').fill('Brook and Pine')
  await page.getByLabel('Based in').fill('Portland')
  await page.getByLabel('Tagline').fill('Small-batch ceramics')
  await page.getByLabel('Description').fill('Pots and mugs thrown by hand.')
  await page.getByLabel('Ships to (country codes)').fill('US, CA')
  await page.getByLabel('Processing time').fill('Ships in 2 days')
  await page.locator('input[type=file][aria-label="Add avatar"]').setInputFiles(image('avatar.png'))
  await expect(page.getByRole('img', { name: 'Store avatar' })).toBeVisible()
  await expect(page.locator('.ob-payment-rails li.is-on')).toContainText('Bitcoin, paid to you directly')
  await expect(page.locator('.ob-payment-rails')).toContainText('Moderated escrow')
  await page.getByRole('button', { name: 'Open my store' }).click()

  await expect(page.getByRole('heading', { name: 'Brook and Pine', level: 1 })).toBeVisible()
  await expect(page.getByText('Small-batch ceramics')).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Your store' })).toBeVisible()
})

test('the seller lists, edits, and removes goods', async ({ page }) => {
  await page.goto(SELLER_UI)
  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Listings' }).click()
  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title').fill('Ash glaze mug')
  await page.getByLabel('Description').fill('Wheel-thrown, 12 oz.')
  await page.getByLabel('Category').fill('Home')
  // Sats keep this test off live exchange rates; fiat pricing is covered by the node tests.
  await page.getByLabel('Price currency').selectOption('SAT')
  await page.getByLabel('Price (sats)').fill('25000')
  await page.getByLabel('Shipping (sats)').fill('3000')
  await page.getByLabel('Ships from').fill('US')
  await page.getByLabel('In stock').fill('5')
  await page.locator('input[type=file][aria-label="Add photos"]').setInputFiles(image('mug.png'))
  await expect(page.getByRole('img', { name: 'Photo 1' })).toBeVisible()
  await page.getByRole('button', { name: 'Publish listing' }).click()
  await expect(page.getByText('Listing published')).toBeVisible()
  const row = page.locator('.ob-own-listing', { hasText: 'Ash glaze mug' })
  await expect(row).toContainText('5 in stock')

  await page.getByRole('button', { name: 'Edit Ash glaze mug' }).click()
  await page.getByLabel('In stock').fill('4')
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(row).toContainText('4 in stock')

  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title').fill('Glaze recipe card')
  await page.getByLabel('Category').fill('Paper')
  await page.getByText('Digital good').click()
  await page.getByLabel('Price currency').selectOption('SAT')
  await page.getByLabel('Price (sats)').fill('2000')
  await page.getByRole('button', { name: 'Publish listing' }).click()
  await expect(page.locator('.ob-own-listing', { hasText: 'Glaze recipe card' })).toBeVisible()
  await page.getByRole('button', { name: 'Remove Glaze recipe card' }).click()
  await page.getByRole('button', { name: 'Remove listing' }).click()
  await expect(page.locator('.ob-own-listing', { hasText: 'Glaze recipe card' })).toHaveCount(0)

  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Storefront' }).click()
  await expect(page.getByRole('button', { name: 'View Ash glaze mug' })).toBeVisible()
  await page.getByRole('button', { name: 'View Ash glaze mug' }).click()
  await expect(page.getByText('This is your listing.')).toBeVisible()
})

test('a buyer follows the store and orders; the seller sees the sale', async ({ page }) => {
  await page.goto('/')
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Followed stores' }).click()
  await page.getByLabel('Store node address').fill(SELLER_NODE)
  await page.getByRole('button', { name: 'Follow store' }).click()
  await expect(page.locator('.ob-follow-list')).toContainText('Brook and Pine')
  await expect(page.locator('.ob-follow-list')).toContainText('1 listing')

  await page.getByRole('button', { name: 'Products', exact: true }).click()
  await page.getByLabel('Search listings', { exact: true }).fill('Ash glaze mug')
  await page.getByRole('button', { name: 'View Ash glaze mug' }).click()
  await expect(page.getByText('Sold by')).toContainText('Brook and Pine')
  await expect(page.getByRole('img', { name: 'Ash glaze mug' }).first()).toBeVisible()
  const addresses = page.waitForResponse(r => new URL(r.url()).pathname === '/addresses')
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await addresses
  // Other specs save a default address on the shared buyer node.
  const saved = page.locator('.ob-address-picker select')
  if (await saved.isVisible()) await saved.selectOption('')
  await page.getByLabel('Full name', { exact: true }).fill('Ada Buyer')
  await page.getByLabel('Street address', { exact: true }).fill('1 Test Lane')
  await page.getByLabel('City', { exact: true }).fill('Portland')
  await page.getByLabel('State / region', { exact: true }).fill('OR')
  await page.getByLabel('Postal code', { exact: true }).fill('97201')
  const country = page.getByLabel('Country', { exact: true })
  if (await country.isVisible()) await country.fill('US')
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Order created', exact: true })).toBeVisible()
  // The seller's node prices the order and replies with a fresh address.
  await expect(page.locator('.ob-payment-progress strong')).toHaveText('Awaiting payment', { timeout: 20000 })
  await expect(page.getByText('Pay from your node wallet')).toBeVisible()

  const seller = await page.context().newPage()
  await seller.goto(SELLER_UI)
  const sales = seller.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Sales' })
  await sales.click()
  const sale = seller.locator('.ob-order-row', { hasText: 'Ash glaze mug' })
  await expect(sale).toContainText('Awaiting payment')
  await sale.getByRole('button').first().click()
  await expect(seller.locator('.ob-sale-address')).toContainText('Ada Buyer')
  await expect(seller.getByText('Ship once the payment confirms.')).toBeVisible()
  await seller.close()
})
