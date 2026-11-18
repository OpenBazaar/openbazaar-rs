import { test, expect, photos } from './harness.ts'
import type { Page } from '@playwright/test'

async function ownListings(seller: { api: (m: string, p: string) => Promise<any> }) {
  const body = await seller.api('GET', '/store/listings')
  return (Array.isArray(body) ? body : body.listings) as { title: string; images: string[] }[]
}

async function openListings(page: Page) {
  await page.getByRole('navigation', { name: 'Your store' }).getByRole('button', { name: 'Listings' }).click()
}

test('a listing is published with several photos, one removed before publishing', async ({ page, seller }) => {
  await page.goto(seller.ui)
  await openListings(page)
  await page.getByRole('button', { name: 'New listing' }).click()
  await page.getByLabel('Title', { exact: true }).fill('Ash glaze mug')
  await page.getByLabel('Description', { exact: true }).fill('Stoneware, 12 oz.')
  await page.getByLabel('Category', { exact: true }).fill('Home')
  await page.getByLabel('Price currency').selectOption('SAT')
  await page.getByRole('textbox', { name: /^Price \(sats\)/ }).fill('25000')
  await page.getByLabel('Ships from', { exact: true }).fill('US')
  await page.locator('input[type=file][aria-label="Add photos"]').setInputFiles(photos(4, 'mug'))
  const media = page.getByRole('group', { name: 'Product media' })
  await expect(media.getByRole('img')).toHaveCount(4)
  const kept = await media.getByRole('img').evaluateAll(imgs => imgs.map(i => (i as HTMLImageElement).src))
  await media.getByRole('button', { name: 'Remove photo 2' }).click()
  await expect(media.getByRole('img')).toHaveCount(3)
  expect(kept.length).toBe(4)
  await page.getByRole('button', { name: 'Publish listing' }).click()
  await expect(page.getByRole('button', { name: 'Edit Ash glaze mug' })).toBeVisible()
  const [mug] = (await ownListings(seller)).filter(l => l.title === 'Ash glaze mug')
  expect(mug.images).toHaveLength(3)
})

test('photos are added to and removed from an existing listing', async ({ page, seller }) => {
  const first = await seller.uploadImage()
  const second = await seller.uploadImage()
  await seller.addListing({ title: 'Linen tea towel', images: [first, second] })
  await page.goto(seller.ui)
  await openListings(page)
  await page.getByRole('button', { name: 'Edit Linen tea towel' }).click()
  const media = page.getByRole('group', { name: 'Product media' })
  await expect(media.getByRole('img')).toHaveCount(2)
  await media.getByRole('button', { name: 'Remove photo 1' }).click()
  await page.locator('input[type=file][aria-label="Add photos"]').setInputFiles(photos(2, 'towel'))
  await expect(media.getByRole('img')).toHaveCount(3)
  await page.getByRole('button', { name: 'Save changes' }).click()
  await expect(page.getByRole('button', { name: 'Edit Linen tea towel' })).toBeVisible()
  const [towel] = (await ownListings(seller)).filter(l => l.title === 'Linen tea towel')
  expect(towel.images).toHaveLength(3)
  expect(towel.images[0]).toBe(second)
  expect(towel.images).not.toContain(first)
})
