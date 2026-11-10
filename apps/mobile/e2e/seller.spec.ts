import { test, expect, type Page } from '@playwright/test'

// Two real disposable nodes (see server.mjs): a fresh seller node and the
// shop's buyer node. The seller publishes a listing. Followed stores takes a
// store key, not a node address.
const SELLER_UI = `http://127.0.0.1:${process.env.OB_E2E_SELLER_UI_PORT || 48174}`
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
  await page.getByLabel('Search', { exact: true }).fill('does not exist')
  await expect(page.getByText('No listings match these filters', { exact: false })).toBeVisible()
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click()
  await expect(row).toBeVisible()
  await page.getByRole('combobox', { name: 'Stock', exact: true }).selectOption('empty')
  await expect(row).toHaveCount(0)
  await page.getByRole('combobox', { name: 'Stock', exact: true }).selectOption('all')
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

test('followed stores take a store key and no address', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByLabel('Store node address')).toHaveCount(0)
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Followed stores' }).click()
  await expect(page.getByLabel('Store node address')).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Open the test network store' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Follow store' })).toHaveCount(0)
  await expect(page.getByLabel('Store key', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Add store', exact: true })).toBeVisible()
})

// A page the node serves to a signed-in browser signs every request, an image
// upload too (src/homeNode/transport.ts). The node's answers are mocked.
test('a browser signed in on the node’s web address uploads a store avatar', async ({ page }) => {
  const hash = 'c414cd0e204de974f73753c7e28d7638e7b3691bb8b1a2bab6b25bb7fed7ce77'
  const uploads: (string | undefined)[] = []
  await page.addInitScript(() => localStorage.setItem('ob-home-device', '01'.repeat(32)))
  await page.route('**/', async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replace('<head>', '<head><meta name="ob-home-node" content="true">') })
  })
  await page.route('**/remote/hello', route => route.fulfill({ json: { nodeId: '90'.repeat(32) } }))
  await page.route('**/remote/devices', route => route.fulfill({ json: { devices: [] } }))
  await page.route('**/store/profile', route => route.fulfill({ json: { storeKey: '34'.repeat(32), role: 'seller', name: 'My store', network: 'testnet4' } }))
  await page.route('**/media', async route => {
    const headers = await route.request().allHeaders()
    uploads.push(headers['x-ob-signature'] && headers['content-type'])
    return route.fulfill({ json: { hash, contentType: 'image/png' } })
  })
  await page.route(`**/media/${hash}`, route => route.fulfill({ contentType: 'image/png', body: PNG }))
  await page.goto('/')
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Store settings' }).click()
  await page.locator('input[type=file][aria-label="Add avatar"]').setInputFiles(image('avatar.png'))
  await expect(page.getByRole('img', { name: 'Store avatar' })).toBeVisible()
  await expect(page.getByRole('alert')).toHaveCount(0)
  expect(uploads).toEqual(['image/png'])
})

// The node keeps images up to 2 MiB and answers a larger upload 413 with a
// plain-text body. Both read as the image being too large, next to the control.
test('an image over the node’s size cap says so next to the control', async ({ page }) => {
  const tooLarge = 'That image is larger than 2 MB. Choose a smaller one.'
  let uploads = 0
  await page.addInitScript(() => localStorage.setItem('ob-home-device', '01'.repeat(32)))
  await page.route('**/', async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replace('<head>', '<head><meta name="ob-home-node" content="true">') })
  })
  await page.route('**/remote/hello', route => route.fulfill({ json: { nodeId: '90'.repeat(32) } }))
  await page.route('**/remote/devices', route => route.fulfill({ json: { devices: [] } }))
  await page.route('**/store/profile', route => route.fulfill({ json: { storeKey: '34'.repeat(32), role: 'seller', name: 'My store', network: 'testnet4' } }))
  await page.route('**/media', route => {
    uploads += 1
    return route.fulfill({ status: 413, contentType: 'text/plain', body: 'request body too large' })
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Store settings' }).click()
  const avatar = page.locator('input[type=file][aria-label="Add avatar"]')
  await avatar.setInputFiles({ name: 'big.png', mimeType: 'image/png', buffer: Buffer.alloc(2 * 1024 * 1024 + 1) })
  await expect(page.getByRole('alert')).toHaveText(tooLarge)
  expect(uploads).toBe(0)
  await avatar.setInputFiles(image('avatar.png'))
  await expect.poll(() => uploads).toBe(1)
  await expect(page.getByRole('alert')).toHaveText(tooLarge)
})
