import { test, expect, PASSPHRASE, type LightningMarket } from './harness.ts'
import type { Page } from '@playwright/test'

// Lightning on a local regtest bitcoind, with no LSP: the seller turns Lightning
// on for its store, the buyer opens a channel to the seller from Settings, and
// pays an order from its Lightning balance. Skips without bitcoind.
test.setTimeout(180000)

async function lightningSettings(page: Page) {
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Lightning', exact: true }).click()
}

/** Seller takes Lightning, buyer has a usable channel to it, and an order is placed with the Lightning radio. */
async function lightningOrder(page: Page, { seller, buyer, sellerPeer, bitcoind }: LightningMarket) {
  await page.goto(seller.ui)
  await lightningSettings(page)
  // https://github.com/OpenBazaar/openbazaar-rs/issues/175: the own store goes by its profile name.
  await page.getByRole('switch', { name: 'Brook and Pine Your store', exact: true }).click()
  await expect.poll(async () => (await seller.api('GET', '/store/profile')).acceptsLightning).toBe(true)

  // The buyer funds its channel; the seller keeps an on-chain reserve, without
  // which its node refuses an inbound anchor channel.
  await bitcoind.send((await buyer.api('POST', '/ln/address')).address, 0.05)
  await bitcoind.send((await seller.api('POST', '/ln/address')).address, 0.001)
  await bitcoind.mine(6)
  for (const node of [buyer, seller])
    await expect.poll(async () => (await node.api('GET', '/ln/status')).balances.onchainSpendableSats, { timeout: 60000 }).toBeGreaterThan(0)

  await page.goto(buyer.ui)
  await lightningSettings(page)
  // The buyer never set up a store, so there is no store of its own to switch on.
  await expect(page.getByText('Your store shows here once it is open.')).toBeVisible()
  await expect(page.getByRole('switch', { name: /Your store/ })).toHaveCount(0)
  const channels = page.getByRole('region', { name: 'Channels' })
  await channels.getByRole('textbox', { name: 'Peer' }).fill(sellerPeer)
  await channels.getByRole('spinbutton', { name: 'Sats' }).fill('1000000')
  await channels.getByRole('button', { name: 'Open channel' }).click()
  const open = page.getByRole('dialog', { name: 'Open a 1,000,000-sat channel' })
  await open.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await open.getByRole('button', { name: 'Confirm' }).click()
  await expect(open).toBeHidden()
  await expect.poll(async () => (await bitcoind.mempool()).length, { timeout: 60000 }).toBeGreaterThan(0)
  await bitcoind.mine(6)
  for (const node of [buyer, seller])
    await expect.poll(async () => (await node.api('GET', '/ln/status')).channels[0]?.usable, { timeout: 90000 }).toBe(true)

  await seller.addListing({ title: 'Field notes zine', kind: 'digital', price: 15000 })
  await buyer.follow(seller)
  await page.reload()
  await page.getByRole('button', { name: 'Products', exact: true }).click()
  await page.getByLabel('Search stores and listings', { exact: true }).fill('Field notes zine')
  await page.getByRole('button', { name: 'View Field notes zine', exact: true }).click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Send to this test node.')
  await page.getByRole('radio', { name: /^Lightning/ }).check()
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Order created' })).toBeVisible()
}

test('a buyer opens a channel to the seller and pays an order over Lightning', async ({ page, lightning }) => {
  const { seller, buyer } = lightning
  await lightningOrder(page, lightning)
  // The seller's node answers with an invoice; the order page then offers Lightning.
  await expect.poll(async () => (await buyer.api('GET', '/orders')).orders[0]?.payment?.rail, { timeout: 30000 }).toBe('ln')
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Your orders' }).click()
  await page.getByRole('button', { name: /^Order from Brook and Pine/ }).first().click()
  const payment = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  await payment.getByRole('button', { name: "Pay from your node's Lightning balance" }).click({ timeout: 15000 })
  const confirm = page.getByRole('dialog', { name: /Confirm/ })
  await expect(confirm).toContainText('15,000')
  await confirm.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await confirm.getByRole('button', { name: /^Confirm/ }).click()
  await expect(confirm).toBeHidden()
  await expect(payment).toContainText(/Paid/, { timeout: 60000 })
  await expect.poll(async () => (await seller.api('GET', '/sales')).sales[0]?.status, { timeout: 30000 }).toMatch(/paid/i)
  await expect.poll(async () => (await buyer.api('GET', '/ln/status')).channels[0]?.outboundSats, { timeout: 30000 }).toBeLessThan(990000 - 14000)
})

test('the Order created page offers the Lightning invoice once the seller sends it', async ({ page, lightning, guard }) => {
  guard.allowOpenDialog()
  await lightningOrder(page, lightning)
  await expect.poll(async () => (await lightning.buyer.api('GET', '/orders')).orders[0]?.payment?.rail, { timeout: 30000 }).toBe('ln')
  const payment = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  await expect(payment.getByRole('button', { name: "Pay from your node's Lightning balance" })).toBeVisible({ timeout: 20000 })
  await expect(payment.getByRole('alert')).toHaveCount(0)
})
