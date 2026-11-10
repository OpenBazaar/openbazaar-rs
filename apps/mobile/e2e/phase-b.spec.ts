import { test, expect } from '@playwright/test'

test('status, sample stores, and add store by key only', async ({ page }) => {
  const followBodies: string[] = []
  page.on('request', request => {
    if (new URL(request.url()).pathname === '/follow') followBodies.push(request.postData() ?? '')
  })
  await page.goto('/')
  await expect(page.getByRole('heading', { name: /^All products/ })).toBeVisible()
  const status = page.getByRole('status')
  await expect(status).toContainText('Testnet4 · test coins, no value')
  await expect(status).toContainText(/Chain (syncing|sync failed|height)/)
  await expect(status).not.toContainText('.onion')
  await expect(page.locator('.ob-sample-badge').first()).toHaveText('Sample')
  await expect(page.getByLabel('Store node address')).toHaveCount(0)
  await expect(page.getByLabel('Store key')).toHaveCount(0)
  await expect(page.getByLabel('Listing title')).toHaveCount(0)
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Followed stores' }).click()
  await expect(page.getByRole('button', { name: 'Open the test network store' })).toHaveCount(0)
  const storeKey = 'ab'.repeat(32)
  await page.getByLabel('Store key', { exact: true }).fill(storeKey)
  await page.getByRole('button', { name: 'Add store', exact: true }).click()
  await expect.poll(() => followBodies.length).toBe(1)
  expect(JSON.parse(followBodies[0])).toEqual({ storeId: storeKey })
})
