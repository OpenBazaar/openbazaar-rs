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
  await page.getByLabel('Display name').fill('Brook and Pine')
  await expect(page.getByText('Saved name: My store')).toBeVisible()
  await page.getByLabel('Based in').fill('Portland')
  await page.getByLabel('Tagline').fill('Small-batch ceramics')
  await page.getByLabel('Description').fill('Pots and mugs thrown by hand.')
  await page.getByLabel('Ships to (country codes)').fill('US, CA')
  await page.getByLabel('Processing time').fill('Ships in 2 days')
  await page.locator('input[type=file][aria-label="Add avatar"]').setInputFiles(image('avatar.png'))
  await expect(page.getByRole('img', { name: 'Store avatar' })).toBeVisible()
  await expect(page.locator('.ob-payment-rails li.is-on')).toContainText('Bitcoin')
  await expect(page.locator('.ob-payment-rails')).toContainText('Moderated escrow')
  await page.getByRole('button', { name: 'Open my store' }).click()

  await expect(page.getByRole('heading', { name: 'Brook and Pine', level: 1 })).toBeVisible()
  await page.getByRole('button', { name: 'Edit store' }).click()
  await expect(page.getByText('Saved name: Brook and Pine')).toBeVisible()
  await page.getByLabel('Display name').fill('   ')
  await expect(page.getByText('Give the store a name.')).toBeVisible()
  await expect(page.getByRole('button', { name: 'Save store' })).toBeDisabled()
  await page.getByLabel('Display name').fill('Brook and Pine')
  await page.getByRole('button', { name: 'View your store' }).click()
  await expect(page.getByText('Small-batch ceramics')).toBeVisible()
  await expect(page.getByRole('navigation', { name: 'Your store' })).toBeVisible()
})

test('the seller lists, edits, and removes goods', async ({ page }, info) => {
  await page.goto(SELLER_UI)
  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Listings' }).click()
  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title').fill('Ash glaze mug')
  await page.getByLabel('Description').fill('Wheel-thrown, 12 oz.')
  await page.getByLabel('Category', { exact: true }).fill('Home')
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
  await page.getByLabel('Search inventory', { exact: true }).fill('does not exist')
  await expect(page.getByText('No listings match these filters.', { exact: false })).toBeVisible()
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click()
  await expect(row).toBeVisible()
  await page.getByRole('combobox', { name: 'Stock status', exact: true }).selectOption('empty')
  await expect(row).toHaveCount(0)
  await page.getByRole('combobox', { name: 'Stock status', exact: true }).selectOption('all')
  for (const width of [320, 768, 1366]) {
    await page.setViewportSize({ width, height: 900 })
    await row.scrollIntoViewIfNeeded()
    expect(await page.locator('.ob-content').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    const boxes = await row.locator(':scope > *').evaluateAll(elements => elements.map(el => {
      const r = el.getBoundingClientRect()
      return { x: r.x, y: r.y, right: r.right, bottom: r.bottom }
    }))
    for (let a = 0; a < boxes.length; a++) for (let b = a + 1; b < boxes.length; b++) {
      const left = boxes[a], right = boxes[b]
      expect(left.x < right.right - 1 && left.right > right.x + 1 && left.y < right.bottom - 1 && left.bottom > right.y + 1).toBeFalsy()
    }
    await page.screenshot({ path: info.outputPath(`published-listing-${width}.png`) })
  }

  await page.getByRole('button', { name: 'Edit Ash glaze mug' }).click()
  await page.getByLabel('In stock').fill('4')
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(row).toContainText('4 in stock')

  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title').fill('Glaze recipe card')
  await page.getByLabel('Category', { exact: true }).fill('Paper')
  await page.getByRole('radio', { name: 'Digital good', exact: true }).check()
  await page.getByLabel('Price currency').selectOption('SAT')
  await page.getByLabel('Price (sats)').fill('2000')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  const draft = page.locator('.ob-own-listing.is-draft', { hasText: 'Glaze recipe card' })
  await expect(draft).toBeVisible()
  await page.setViewportSize({ width: 320, height: 900 })
  await draft.scrollIntoViewIfNeeded()
  expect((await draft.locator('.ob-own-listing-copy').boundingBox())!.width).toBeGreaterThan(180)
  await page.screenshot({ path: info.outputPath('draft-listing-320.png') })
  await page.setViewportSize({ width: 1366, height: 900 })
  await page.getByRole('button', { name: 'Edit draft Glaze recipe card', exact: true }).click()
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

test('a buyer follows the store and sees the new-seller escrow requirement', async ({ page }) => {
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
  // The current node deliberately refuses direct pay for a new seller.
  // Keep that real policy visible; this is not a successful-sale fixture.
  await expect(page.locator('.ob-payment-progress strong')).toHaveText('Declined by the seller', { timeout: 20000 })
  await expect(page.getByRole('alert').filter({ hasText: 'A new seller takes moderated escrow, with the moderator named.' })).toBeVisible()
  await expect(page.getByText('Pay from your node wallet', { exact: true })).toHaveCount(0)
})
