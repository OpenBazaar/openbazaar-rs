import { test, expect, PASSPHRASE } from './harness.ts'

test('auto-lock minutes change only with the passphrase, and the node keeps them', async ({ page, buyer, guard }) => {
  await page.goto(buyer.ui)
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Wallet' }).click()
  await page.getByRole('button', { name: 'Wallet settings' }).click()
  const dialog = page.getByRole('dialog', { name: 'Wallet settings' })
  await dialog.getByText('Lock settings').click()
  await dialog.getByRole('combobox').first().selectOption({ label: '5 minutes' })
  await dialog.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await dialog.getByRole('button', { name: /^Save/ }).click()
  await expect.poll(async () => (await buyer.lockState()).autoLockMinutes).toBe(5)
  await dialog.getByRole('button', { name: 'Lock now' }).click()
  await expect.poll(async () => (await buyer.lockState()).state).toBe('locked')
  guard.allowOpenDialog()
})
