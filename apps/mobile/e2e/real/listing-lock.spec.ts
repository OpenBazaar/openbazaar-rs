import { test, expect, photos, PASSPHRASE } from './harness.ts'

// The wallet locks while a seller is editing (auto-lock, or another window).
// Saving asks for the passphrase in a dialog, and the real passphrase must
// unlock the real node and finish the save. v0.1.0-alpha.7 shipped with this
// dialog refusing the passphrase after adding photos.

async function listings(seller: { api: (m: string, p: string) => Promise<any> }) {
  const body = await seller.api('GET', '/store/listings')
  return (Array.isArray(body) ? body : body.listings) as { title: string; images?: string[]; listingId?: string }[]
}

test('a new listing with photos is published after unlocking mid-edit', async ({ page, seller, guard }) => {
  guard.allowStatus(423, /^\/store\/listings/)
  await page.goto(seller.ui)
  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Listings' }).click()
  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title', { exact: true }).fill('Ash glaze mug')
  await page.getByLabel('Description', { exact: true }).fill('Stoneware, 12 oz.')
  await page.getByLabel('Category', { exact: true }).fill('Home')
  await page.getByRole('textbox', { name: /^Price \(USD\)/ }).fill('24')
  await page.getByLabel('Ships from', { exact: true }).fill('Portland')

  await seller.lock()
  await page.locator('input[type=file][aria-label="Add photos"]').setInputFiles(photos(3, 'mug'))
  await expect(page.getByRole('group', { name: 'Product media' }).getByRole('img')).toHaveCount(3)
  await page.getByRole('button', { name: 'Publish listing' }).click()

  const dialog = page.getByRole('dialog', { name: 'Unlock wallet' })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await dialog.getByRole('button', { name: 'Unlock wallet' }).click()
  await expect(dialog).toBeHidden()
  await expect.poll(async () => (await seller.lockState()).state).toBe('unlocked')
  await expect.poll(async () => (await listings(seller)).find(l => l.title === 'Ash glaze mug')?.images?.length).toBe(3)
})

test('adding photos to an existing listing saves after unlocking mid-edit', async ({ page, seller, guard }) => {
  guard.allowStatus(423, /^\/store\/listings/)
  await seller.addListing({ title: 'Linen tea towel', currency: 'USD', price: 1800, images: [await seller.uploadImage()] })
  await page.goto(seller.ui)
  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Listings' }).click()
  await page.getByRole('button', { name: 'Edit Linen tea towel' }).click()

  await seller.lock()
  await page.locator('input[type=file][aria-label="Add photos"]').setInputFiles(photos(3, 'towel'))
  await page.getByRole('button', { name: 'Save changes' }).click()

  const dialog = page.getByRole('dialog', { name: 'Unlock wallet' })
  await expect(dialog).toBeVisible()
  await dialog.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await dialog.getByRole('button', { name: 'Unlock wallet' }).click()
  await expect(dialog).toBeHidden()
  await expect.poll(async () => (await seller.lockState()).state).toBe('unlocked')
  await expect.poll(async () => (await listings(seller)).find(l => l.title === 'Linen tea towel')?.images?.length).toBe(4)
})
