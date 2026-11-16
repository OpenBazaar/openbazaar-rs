import { test, expect, type Page } from '@playwright/test'

const to = 'tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx'
const txid = 'd'.repeat(64)
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: false, rememberAvailable: true, backupConfirmed: true, minPassphraseChars: 8 }
const wallet = { network: 'testnet4', balance: { spendableSats: 391054, confirmedSats: 391054, pendingSats: 0, totalSats: 391054 }, receiveAddress: `tb1p${'q'.repeat(58)}`, lastSync: 1794272400, syncError: null, activity: [] }
const quote = { quote: 'q1', network: 'testnet4', address: to, amountSats: 25000, max: false, linkCoins: false, feeSats: 282, feeCapSats: 423, feeRateSatVb: 2, totalSats: 25282, expiresInSecs: 60, heldBackSats: 0 }

async function openSend(page: Page) {
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, publicChainServer: true, lightning: 'unavailable', silentPayments: 'code-only', proxy: null } }))
  await page.route('**/wallet', route => route.fulfill({ json: wallet }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: lock }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
  await page.goto('/')
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  await page.getByRole('button', { name: 'Send', exact: true }).click()
  return page.getByRole('dialog', { name: 'Send bitcoin', exact: true })
}

test('bitcoin send: check, review, passphrase, then the txid', async ({ page }, info) => {
  const bodies: Record<string, unknown>[] = []
  await page.route('**/wallet/send/quote', route => { bodies.push(route.request().postDataJSON()); return route.fulfill({ json: quote }) })
  await page.route('**/wallet/send', route => { bodies.push(route.request().postDataJSON()); return route.fulfill({ json: { ...quote, txid } }) })
  const dialog = await openSend(page)
  await expect(dialog).toContainText('Testnet4 · test coins, no value')

  await dialog.getByLabel('Recipient address').fill('bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4')
  await dialog.getByLabel('Amount in sats').fill('25,000')
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('That is a mainnet address. This wallet holds testnet4 coins')
  expect(bodies).toHaveLength(0)
  await page.screenshot({ path: info.outputPath('send-form-error.png') })

  await dialog.getByLabel('Recipient address').fill(to)
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('282 sats at 2 sat/vB (never more than 423 sats)')
  await expect(dialog).toContainText('25,282 sats')
  expect(bodies[0]).toEqual({ address: to, amountSats: 25000 })
  const send = dialog.getByRole('button', { name: 'Send test bitcoin', exact: true })
  await expect(send).toBeDisabled()
  await page.screenshot({ path: info.outputPath('send-review.png') })

  await dialog.getByLabel('Wallet passphrase').fill('correct horse battery')
  await send.click()
  await expect(dialog).toContainText('Sent 25,000 sats with a 282 sats network fee.')
  await expect(dialog).toContainText(txid)
  expect(bodies[1]).toEqual({ quote: 'q1', passphrase: 'correct horse battery', confirm: true })
  await page.screenshot({ path: info.outputPath('send-result.png') })
})

test('bitcoin send: max, a used quote, and an unreachable network', async ({ page }) => {
  const bodies: Record<string, unknown>[] = []
  let sends = 0
  await page.route('**/wallet/send/quote', route => { bodies.push(route.request().postDataJSON()); return route.fulfill({ json: { ...quote, max: true, amountSats: 390772, totalSats: 391054 } }) })
  await page.route('**/wallet/send', route => ++sends === 1
    ? route.fulfill({ status: 409, json: { error: 'This review has expired or was already used. Review the send again.' } })
    : route.fulfill({ status: 502, json: { error: 'broadcast failed' } }))
  const dialog = await openSend(page)
  await dialog.getByLabel('Recipient address').fill(to)
  await dialog.getByLabel('Send max').check()
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  expect(bodies[0]).toEqual({ address: to, max: true })
  await expect(dialog).toContainText('everything you can spend')
  await dialog.getByLabel('Wallet passphrase').fill('correct horse battery')
  await dialog.getByRole('button', { name: 'Send test bitcoin', exact: true }).click()
  await expect(dialog).toContainText('Review the send again')
  await dialog.getByRole('button', { name: 'Review again', exact: true }).click()
  await dialog.getByRole('button', { name: 'Send test bitcoin', exact: true }).click()
  await expect(dialog).toContainText('could not reach the network, so nothing was sent')
  expect(bodies).toHaveLength(2)
})

test('bitcoin send: not enough funds is explained before review', async ({ page }) => {
  await page.route('**/wallet/send/quote', route => route.fulfill({ status: 400, json: { error: 'Not enough funds: this needs 500,282 sats including the network fee, and the wallet can spend 391,054 sats now.' } }))
  const dialog = await openSend(page)
  await dialog.getByLabel('Recipient address').fill(to)
  await dialog.getByLabel('Amount in sats').fill('500000')
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('Not enough funds')
  await expect(dialog.getByRole('button', { name: 'Review', exact: true })).toBeVisible()
})

test('zcash says sending is not available yet', async ({ page }, info) => {
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, publicChainServer: false, lightning: 'unavailable', silentPayments: 'code-only', zecBackend: { enabled: true, backend: 'own-node', network: 'testnet', privacy: 'own-node', reveals: 'Your own indexer reads the shielded chain.', servers: ['http://127.0.0.1:8137'], active: null, software: null, proxied: false, tip: 4000000, scannedHeight: 4000000, error: null } } }))
  await page.route('**/wallet', route => route.fulfill({ json: wallet }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: lock }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
  await page.goto('/')
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  await page.getByRole('tab', { name: /^Zcash/ }).click()
  const send = page.getByRole('tabpanel').getByRole('button', { name: 'Send', exact: true })
  await expect(send).toBeDisabled()
  await expect(send).toHaveAccessibleDescription('Sending Zcash isn’t available yet.')
  await page.getByRole('tabpanel').screenshot({ path: info.outputPath('send-zcash-unavailable.png') })
})

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
