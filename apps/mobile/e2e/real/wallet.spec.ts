import { test, expect, freePort } from './harness.ts'
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

// https://github.com/OpenBazaar/openbazaar-rs/issues/174
test('an unreachable chain server gets a plain, retryable message on the wallet page', async ({ page, spawn }) => {
  const node = await spawn('offline-chain', { role: 'buyer', chainUrl: `http://127.0.0.1:${await freePort()}/chain` })
  await node.setupWallet()
  await expect.poll(async () => (await node.api('GET', '/wallet')).syncError, { timeout: 20000 }).toBeTruthy()
  await page.goto(node.ui)
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Wallet' }).click()
  const alert = page.getByRole('region', { name: 'Wallet balance' }).getByRole('alert')
  await expect(alert).toContainText('Balance may be out of date. Your chain server could not be reached.')
  await expect(alert).not.toContainText(/127\.0\.0\.1|Reqwest|Error/)
  await alert.getByRole('button', { name: 'Try again' }).click()
  await expect(alert).toContainText('Checked again at')
})
