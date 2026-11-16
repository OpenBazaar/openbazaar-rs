import { test, expect } from '@playwright/test'

const to = 'tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx'
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: false, rememberAvailable: true, backupConfirmed: true, minPassphraseChars: 8 }
const wallet = { network: 'testnet4', balance: { spendableSats: 391054, confirmedSats: 391054, pendingSats: 0, totalSats: 391054 }, receiveAddress: `tb1p${'q'.repeat(58)}`, lastSync: 1794272400, syncError: null, activity: [] }

test('lightning: pay a pasted invoice, and withdraw on-chain funds to a Mutinynet address', async ({ page }, info) => {
  const ln = { enabled: true, network: 'signet', running: true, keys: 'ready', nodeId: `03${'b'.repeat(64)}`, balances: { lightningSats: 345340, inboundSats: 47000, onchainSats: 199616 }, channels: [], reveals: ['Lightning peers see your node ID and payment routing information.'] }
  const invoice = `lntbs200u1pj${'q'.repeat(180)}`
  const bodies: [string, Record<string, unknown>][] = []
  const record = (route: import('@playwright/test').Route, json: object) => { bodies.push([new URL(route.request().url()).pathname, route.request().postDataJSON()]); return route.fulfill({ json }) }
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, publicChainServer: false, lightning: 'available', silentPayments: 'code-only' } }))
  await page.route('**/wallet', route => route.fulfill({ json: wallet }))
  await page.route('**/ln/status', route => route.fulfill({ json: ln }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: lock }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
  await page.route('**/ln/send/quote', route => record(route, { quote: 'l1', network: 'signet', paymentHash: 'f'.repeat(64), amountMsat: 20000000, amountSats: 20000, feeCapMsat: 200000, feeCapSats: 200, payee: `02${'d'.repeat(64)}`, description: 'Coffee beans', expiresInSecs: 60 }))
  await page.route('**/ln/send', route => record(route, { network: 'signet', paymentHash: 'f'.repeat(64), amountSats: 20000, feeCapSats: 200, status: 'paid', feeSats: 3 }))
  await page.route('**/ln/withdraw/quote', route => record(route, { quote: 'w1', network: 'signet', address: to, amountSats: 199616, max: true, expiresInSecs: 60 }))
  await page.route('**/ln/withdraw', route => record(route, { network: 'signet', address: to, amountSats: 199616, max: true, txid: '9'.repeat(64) }))
  await page.goto('/')
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  await page.getByRole('tab', { name: /^Lightning/ }).click()
  const panel = page.getByRole('tabpanel')

  await panel.getByRole('button', { name: 'Pay invoice', exact: true }).click()
  let dialog = page.getByRole('dialog', { name: 'Pay a Lightning invoice', exact: true })
  await expect(dialog).toContainText('Mutinynet signet · test coins, no value')
  await dialog.getByLabel('Lightning invoice').fill(`lnbc200u1pj${'q'.repeat(180)}`)
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('That invoice is for mainnet')
  await dialog.getByLabel('Lightning invoice').fill(invoice)
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('Coffee beans')
  await expect(dialog).toContainText('At most 20,200 sats')
  await page.screenshot({ path: info.outputPath('send-ln-review.png') })
  await dialog.getByLabel('Wallet passphrase').fill('correct horse battery')
  await dialog.getByRole('button', { name: 'Pay invoice', exact: true }).click()
  await expect(dialog).toContainText('Paid 20,000 sats with a 3 sats routing fee.')
  await expect(dialog).toContainText('f'.repeat(64))
  await page.screenshot({ path: info.outputPath('send-ln-result.png') })
  await dialog.getByRole('button', { name: 'Done', exact: true }).click()

  await panel.getByRole('button', { name: 'Withdraw', exact: true }).click()
  dialog = page.getByRole('dialog', { name: 'Withdraw on-chain funds', exact: true })
  await expect(dialog).toContainText('Your bitcoin wallet is on testnet4, so it cannot receive them.')
  await dialog.getByLabel('Recipient address on Mutinynet').fill(to)
  await dialog.getByLabel('Send max').check()
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('Everything on chain (now 199,616 sats)')
  await page.screenshot({ path: info.outputPath('send-ln-withdraw-review.png') })
  await dialog.getByLabel('Wallet passphrase').fill('correct horse battery')
  await dialog.getByRole('button', { name: 'Withdraw', exact: true }).click()
  await expect(dialog).toContainText('9'.repeat(64))
  expect(bodies).toEqual([
    ['/ln/send/quote', { invoice }],
    ['/ln/send', { quote: 'l1', passphrase: 'correct horse battery', confirm: true }],
    ['/ln/withdraw/quote', { address: to, max: true }],
    ['/ln/withdraw', { quote: 'w1', passphrase: 'correct horse battery', confirm: true }]
  ])
})
