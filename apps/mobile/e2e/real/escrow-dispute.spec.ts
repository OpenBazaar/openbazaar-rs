import { test, expect, PASSPHRASE } from './harness.ts'
import type { Page } from '@playwright/test'
import type { Chain, Market } from './harness.ts'

/** Funds the buyer, orders with moderated escrow from the seller, pays, and waits for the escrow to be funded. */
async function fundedEscrow(page: Page, { seller, buyer }: Market, chain: Chain, moderator: RegExp) {
  chain.fund(await buyer.receiveAddress(), 200000)
  await expect.poll(async () => (await buyer.balance()).spendableSats, { timeout: 20000 }).toBe(200000)
  await seller.addListing({ title: 'Field notes zine', kind: 'digital', price: 15000 })
  await buyer.follow(seller)
  await page.goto(buyer.ui)
  await page.getByLabel('Search stores and listings', { exact: true }).fill('Field notes zine')
  await page.getByRole('button', { name: 'View Field notes zine', exact: true }).click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Send to this test node.')
  await page.getByRole('radio', { name: /^Moderated escrow/ }).check()
  const choice = page.getByRole('group', { name: 'Choose a moderator for Brook and Pine' })
  await choice.getByRole('radio', { name: moderator }).check()
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  const payment = page.getByRole('region', { name: 'Payment for Brook and Pine' })
  await payment.getByRole('button', { name: 'Pay from your node wallet', exact: true }).click({ timeout: 30000 })
  const confirm = page.getByRole('dialog', { name: 'Confirm payment' })
  await confirm.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await confirm.getByRole('button', { name: 'Confirm payment', exact: true }).click()
  await expect.poll(() => chain.mempool().length, { timeout: 20000 }).toBe(1)
  const funding: string = chain.mempool()[0]
  chain.mine(1)
  await expect.poll(async () => (await seller.api('GET', '/escrow/sales')).sales[0]?.stage, { timeout: 30000 }).toBe('funded')
  return { payment, funding }
}

async function dispute(page: Page, payment: ReturnType<Page['getByRole']>, { moderator }: Market) {
  await payment.getByText('Open a dispute', { exact: true }).click()
  await payment.getByRole('textbox', { name: 'What went wrong' }).fill('The zine never arrived.')
  await payment.getByRole('button', { name: 'Send dispute letter' }).click()
  await expect(payment.getByRole('region', { name: 'Dispute filing fee' })).toBeVisible()
  await expect.poll(async () => (await moderator.api('GET', '/moderator/disputes')).disputes.length, { timeout: 30000 }).toBe(1)
}


// Moderated escrow across three real nodes: the buyer funds a 2-of-3 escrow
// from its wallet, disputes, the moderator rules from its desk, and the
// settlement confirms on the simulated chain.
test('an escrow order is disputed, ruled for the buyer, and settled on chain', async ({ page, market, chain }) => {
  const { moderator, buyer } = market
  const { payment } = await fundedEscrow(page, market, chain, /Built-in test moderator/)
  await dispute(page, payment, market)

  // The moderator's wallet locked while the case waited: review works, signing needs the passphrase.
  await moderator.lock()
  await page.goto(moderator.ui)
  await page.getByRole('button', { name: 'Moderator desk' }).click()
  const desk = page.getByRole('region', { name: 'Dispute workspace' })
  await desk.getByRole('button', { name: /^Case / }).first().click()
  await expect(desk.getByRole('button', { name: 'Review ruling for buyer' })).toBeDisabled()
  await page.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  const security = page.getByRole('dialog', { name: 'Wallet security' })
  await expect(security).toContainText('sign a final escrow ruling')
  await security.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
  await security.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  await security.getByRole('button', { name: 'Done', exact: true }).click()
  await expect(security).toBeHidden()
  await desk.getByRole('button', { name: 'Review ruling for buyer' }).click()
  const rule = page.getByRole('dialog', { name: 'Confirm final ruling' })
  await rule.getByRole('button', { name: 'Confirm ruling for buyer' }).click()
  await expect(rule).toBeHidden()
  await expect.poll(async () => (await moderator.api('GET', '/moderator/disputes')).disputes[0].ruled, { timeout: 30000 }).toBe(true)

  await expect.poll(() => chain.mempool().length, { timeout: 45000 }).toBe(1)
  const settlement = chain.tx(chain.mempool()[0])
  chain.mine(1)
  const refund = settlement.vout.reduce((sum: number, o: { value: number }) => sum + o.value, 0)
  expect(refund).toBeGreaterThan(14000)
  await expect.poll(async () => (await buyer.balance()).totalSats, { timeout: 30000 }).toBeGreaterThan(200000 - 15496 - 1000 + 14000)
})

test('a moderator with a filing fee asks for it, the buyer pays it, and the ruling settles', async ({ page, market, chain }) => {
  const { moderator, seller, buyer } = market
  const own = await moderator.api('PUT', '/moderator/profile', { name: 'Ridge Arbitration', feeSats: 2000, description: 'Fast answers.', languages: ['en'], terms: 'Evidence first.' })
  await seller.api('POST', '/moderators', { address: own.address })
  const { payment, funding } = await fundedEscrow(page, market, chain, /Ridge Arbitration/)
  await expect(payment).toContainText(own.id.slice(0, 8))
  await dispute(page, payment, market)

  // The buyer's only coin funded the escrow. Its change is held back from anyone
  // else, and the wallet says so, but the filing fee for this escrow may spend it.
  const held = await buyer.balance()
  expect(held.spendableSats).toBe(0)
  expect(held.heldBackSats).toBeGreaterThan(180000)
  const panel = payment.getByRole('region', { name: 'Dispute filing fee' })
  await expect(panel).toContainText('asks 2,000 sats', { timeout: 45000 })
  await panel.getByRole('button', { name: 'Pay 2,000 sats filing fee' }).click({ timeout: 45000 })
  const fee = page.getByRole('dialog', { name: 'Pay the filing fee' })
  await expect(fee).toContainText('2,000 sats')
  await expect(fee).toContainText('Ridge Arbitration')
  await fee.getByLabel('Wallet passphrase').fill(PASSPHRASE)
  await fee.getByRole('button', { name: 'Confirm payment', exact: true }).click()
  await expect(fee).toBeHidden()
  await expect.poll(() => chain.mempool().length, { timeout: 20000 }).toBe(1)
  const paid = chain.tx(chain.mempool()[0])
  expect(paid.vout.some((o: { value: number }) => o.value === 2000)).toBe(true)
  expect(paid.vin.map((i: { txid: string }) => i.txid)).toEqual([funding])
  chain.mine(1)
  await expect.poll(async () => (await moderator.api('GET', '/moderator/disputes')).disputes[0].fee.state, { timeout: 30000 }).toBe('paid')

  await page.goto(moderator.ui)
  await page.getByRole('button', { name: 'Moderator desk' }).click()
  const desk = page.getByRole('region', { name: 'Dispute workspace' })
  await desk.getByRole('button', { name: /^Case / }).first().click()
  await desk.getByRole('button', { name: 'Review ruling for buyer' }).click()
  const rule = page.getByRole('dialog', { name: 'Confirm final ruling' })
  await rule.getByRole('button', { name: 'Confirm ruling for buyer' }).click()
  await expect(rule).toBeHidden()
  await expect.poll(() => chain.mempool().length, { timeout: 45000 }).toBe(1)
  chain.mine(1)
  await expect.poll(async () => (await buyer.api('GET', '/orders')).orders[0].status, { timeout: 30000 }).toMatch(/refund|settled|ruled/i)
})
