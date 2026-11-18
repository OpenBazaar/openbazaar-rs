import { test, expect, PASSPHRASE } from './harness.ts'
import type { Page } from '@playwright/test'

async function openWallet(page: Page) {
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Wallet' }).click()
  return page.getByRole('region', { name: 'Wallet balance' })
}

test('a locked wallet refuses a wrong passphrase and opens with the right one', async ({ page, buyer, guard }) => {
  guard.allowStatus(400, /^\/unlock$/)
  await buyer.lock()
  await page.goto(buyer.ui)
  const balance = await openWallet(page)
  await expect(balance).toContainText('Locked · receiving available')
  await balance.getByRole('button', { name: 'Unlock wallet' }).click()
  const form = page.locator('form.ob-wallet-lock')
  await expect(form.getByRole('heading', { name: 'Wallet locked' })).toBeVisible()

  await form.getByLabel('Wallet passphrase').fill('not the passphrase')
  await form.getByRole('button', { name: 'Unlock wallet' }).click()
  await expect(form.getByRole('alert')).toBeVisible()
  expect((await buyer.lockState()).state).toBe('locked')

  await form.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await form.getByRole('button', { name: 'Unlock wallet' }).click()
  await expect(form).toBeHidden()
  await expect.poll(async () => (await buyer.lockState()).state).toBe('unlocked')
  guard.allowOpenDialog()
})

test('a wallet locked elsewhere shows as locked when the page loads again', async ({ page, buyer }) => {
  await page.goto(buyer.ui)
  await expect(await openWallet(page)).toContainText('Unlocked · payments require passphrase')
  await buyer.lock()
  await page.reload()
  await expect(await openWallet(page)).toContainText('Locked · receiving available')
})
