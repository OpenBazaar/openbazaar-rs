import { test, expect, PASSPHRASE, type Chain, type TestNode } from './harness.ts'
import type { Page } from '@playwright/test'

// Real direct payments: the buyer's node quotes, the passphrase issues a spend
// ticket, the node signs and broadcasts to the simulated chain, and both nodes
// see the payment confirm once a block is mined.

async function fund(chain: Chain, node: TestNode, sats: number) {
  chain.fund(await node.receiveAddress(), sats)
  await expect.poll(async () => (await node.balance()).spendableSats, { timeout: 20000 }).toBe(sats)
}

async function order(page: Page, seller: TestNode, buyer: TestNode, title: string) {
  await seller.addListing({ title, kind: 'digital', price: 15000 })
  await buyer.follow(seller)
  await page.goto(buyer.ui)
  await page.getByLabel('Search stores and listings', { exact: true }).fill(title)
  await page.getByRole('button', { name: `View ${title}`, exact: true }).click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Send to this test node.')
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  const payment = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  await expect(payment).toContainText('Awaiting payment', { timeout: 30000 })
  return payment
}

async function confirmAndMine(page: Page, chain: Chain, seller: TestNode) {
  const confirm = page.getByRole('dialog', { name: 'Confirm payment' })
  await expect(confirm).toContainText('15,000 sats')
  await confirm.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await confirm.getByRole('button', { name: 'Confirm payment', exact: true }).click()
  await expect(confirm).toBeHidden()
  await expect.poll(() => chain.mempool().length, { timeout: 20000 }).toBe(1)
  chain.mine(1)
  await expect.poll(async () => (await seller.api('GET', '/sales')).sales[0]?.status, { timeout: 30000 }).toMatch(/paid/i)
}

test('a funded buyer pays from the node wallet and both nodes see it paid after a block', async ({ page, chain, seller, buyer }) => {
  await fund(chain, buyer, 200000)
  const payment = await order(page, seller, buyer, 'Field notes zine')
  await payment.getByRole('button', { name: 'Pay from your node wallet', exact: true }).click()
  await confirmAndMine(page, chain, seller)
  await expect(payment).toContainText(/Paid/, { timeout: 30000 })
  await expect.poll(async () => (await buyer.balance()).totalSats, { timeout: 20000 }).toBeLessThan(200000 - 15000)
})

test('a wallet that locked before paying unlocks with the passphrase, then steps up', async ({ page, chain, seller, buyer, guard }) => {
  guard.allowStatus(423, /^\/orders\/[0-9a-f]+\/pay\/quote$/)
  await fund(chain, buyer, 200000)
  const payment = await order(page, seller, buyer, 'Glaze recipe card')
  await buyer.lock()
  await payment.getByRole('button', { name: 'Pay from your node wallet', exact: true }).click()
  const unlock = page.getByRole('dialog', { name: 'Unlock wallet' })
  await expect(unlock).toBeVisible()
  await unlock.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await unlock.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  await expect(unlock).toBeHidden()
  await confirmAndMine(page, chain, seller)
})
