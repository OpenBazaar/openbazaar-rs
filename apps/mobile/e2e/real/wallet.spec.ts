import { test, expect } from './harness.ts'
import type { Page } from '@playwright/test'

async function receiveAddress(page: Page) {
  const fund = page.getByRole('complementary', { name: 'Fund your wallet' })
  return (await fund.locator('code').first().textContent())!.trim()
}

test('coins sent to the receive address show as pending, then spendable once mined', async ({ page, buyer, chain }) => {
  await page.goto(buyer.ui)
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Wallet' }).click()
  const address = await receiveAddress(page)
  expect(address).toMatch(/^tb1/)
  const balance = page.getByRole('region', { name: 'Wallet balance' })

  chain.fund(address, 250000, { confirm: false })
  await expect.poll(async () => (await buyer.balance()).totalSats).toBe(250000)
  await balance.getByRole('button', { name: 'Refresh wallet' }).click()
  await balance.getByRole('button', { name: 'Refresh wallet' }).click()
  await expect(balance.getByRole('article').filter({ hasText: 'Pending confirmation' })).toContainText('250,000', { timeout: 20000 })
  chain.mine(1)
  await expect.poll(async () => (await buyer.balance()).spendableSats).toBe(250000)
  await balance.getByRole('button', { name: 'Refresh wallet' }).click()
  await balance.getByRole('button', { name: 'Refresh wallet' }).click()
  await expect(balance.getByRole('article').filter({ hasText: 'spendable balance' })).toContainText('250,000', { timeout: 20000 })
  await expect(page.getByRole('region', { name: 'Activity' })).toContainText('250,000')
})
