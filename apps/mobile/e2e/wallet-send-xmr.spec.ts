import { test, expect } from '@playwright/test'

const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: false, rememberAvailable: true, backupConfirmed: true, minPassphraseChars: 8 }
const wallet = { network: 'testnet4', balance: { spendableSats: 391054, confirmedSats: 391054, pendingSats: 0, totalSats: 391054 }, receiveAddress: `tb1p${'q'.repeat(58)}`, lastSync: 1794272400, syncError: null, activity: [] }

test('monero send: stagenet address, review in XMR, passphrase, then the txid', async ({ page }, info) => {
  const xmrTo = `7${'b'.repeat(94)}`
  const xmrWallet = { network: 'stagenet', enabled: true, keys: 'ready', primaryAddress: `5${'a'.repeat(94)}`, balance: { unlockedPiconero: '5000000000000', pendingPiconero: '0' }, scannedHeight: 2000000, tip: 2000000, lastSync: 1794560000, syncError: null }
  const xmrQuote = { quote: 'x1', network: 'stagenet', address: xmrTo, amountPiconero: '1250000000000', feePiconero: '30720000', feeCapPiconero: '46080000', totalPiconero: '1250030720000', expiresInSecs: 60 }
  const bodies: Record<string, unknown>[] = []
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, publicChainServer: false, lightning: 'unavailable', silentPayments: 'code-only', xmrBackend: { backend: 'own-node', network: 'stagenet', privacy: 'own-node', reveals: 'Your own Monero node reads blocks.', nodes: ['http://127.0.0.1:38081'], proxied: false, tip: 2000000, scannedHeight: 2000000, error: null } } }))
  await page.route('**/wallet', route => route.fulfill({ json: wallet }))
  await page.route('**/xmr/wallet', route => route.fulfill({ json: xmrWallet }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: lock }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
  await page.route('**/xmr/wallet/send/quote', route => { bodies.push(route.request().postDataJSON()); return route.fulfill({ json: xmrQuote }) })
  await page.route('**/xmr/wallet/send', route => { bodies.push(route.request().postDataJSON()); return route.fulfill({ json: { ...xmrQuote, txid: 'e'.repeat(64) } }) })
  await page.goto('/')
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  await page.getByRole('tab', { name: /^Monero/ }).click()
  await page.getByRole('tabpanel').getByRole('button', { name: 'Send', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Send Monero', exact: true })
  await expect(dialog).toContainText('Stagenet · test coins, no value')
  await expect(dialog.getByLabel('Send max')).toHaveCount(0)
  await dialog.getByLabel('Recipient address').fill(`4${'c'.repeat(94)}`)
  await dialog.getByLabel('Amount in XMR').fill('1.25')
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('That is a mainnet address. This wallet holds stagenet coins')
  await dialog.getByLabel('Recipient address').fill(xmrTo)
  await dialog.getByRole('button', { name: 'Review', exact: true }).click()
  await expect(dialog).toContainText('0.00003072 XMR (never more than 0.00004608 XMR)')
  expect(bodies[0]).toEqual({ address: xmrTo, amountPiconero: '1250000000000' })
  await page.screenshot({ path: info.outputPath('send-xmr-review.png') })
  await dialog.getByLabel('Wallet passphrase').fill('correct horse battery')
  await dialog.getByRole('button', { name: 'Send test Monero', exact: true }).click()
  await expect(dialog).toContainText('Sent 1.25 XMR with a 0.00003072 XMR network fee.')
  expect(bodies[1]).toEqual({ quote: 'x1', passphrase: 'correct horse battery', confirm: true })
  await page.screenshot({ path: info.outputPath('send-xmr-result.png') })
})
