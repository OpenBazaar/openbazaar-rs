import { test, expect, type Page } from '@playwright/test'

const address = `tb1p${'q'.repeat(58)}`
const freshAddress = `tb1p${'z'.repeat(58)}`
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: false, rememberAvailable: true, backupConfirmed: true, minPassphraseChars: 8 }
const activity = [
  { txid: 'a'.repeat(64), direction: 'in', sats: 400000, confirmed: true, height: 160250, time: 1794268200, label: 'Deposit' },
  { txid: 'b'.repeat(64), direction: 'out', sats: 8946, confirmed: true, height: 160251, time: 1794271800, label: 'Marlow Goods' },
  { txid: 'c'.repeat(64), direction: 'in', sats: 20000, confirmed: false, height: null, time: 1794272400, label: 'Deposit' }
]
const wallet = { network: 'testnet4', balance: { spendableSats: 391054, confirmedSats: 391054, pendingSats: 20000, totalSats: 411054 }, receiveAddress: address, lastSync: 1794272400, syncError: null, activity }

async function openWallet(page: Page) {
  await page.goto('/')
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Your wallet', exact: true })).toBeVisible()
}
async function fixture(page: Page, network = 'testnet4', locked = false) {
  await page.route('**/network', route => route.fulfill({ json: { network, confirmations: 1, publicChainServer: true, lightning: 'unavailable', silentPayments: 'code-only', proxy: 'socks5h://127.0.0.1:9050', chainBackend: { backend: 'compact-filters', privacy: 'public', scannedHeight: 160251, peers: 0, fallbackActive: true, reveals: 'Tip, fees and broadcast transactions.' } } }))
  await page.route('**/wallet', route => route.fulfill({ json: { ...wallet, network } }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: { ...lock, state: locked ? 'locked' : 'unlocked' } }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
}

for (const [width, dark] of [[320, false], [768, true], [1366, false], [1920, true]] as const) {
  test(`wallet overview and receive flow fit ${width}px`, async ({ page, context }, info) => {
    await page.setViewportSize({ width, height: 900 })
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await fixture(page)
    let addressCalls = 0
    await page.route('**/wallet/address', route => { addressCalls++; return route.fulfill({ json: { network: 'testnet4', address: freshAddress } }) })
    await openWallet(page)
    if (await page.getByRole('switch', { name: 'Dark mode', exact: true }).isChecked() !== dark) await page.getByRole('switch', { name: 'Dark mode', exact: true }).click()
    const summary = page.getByRole('region', { name: 'Wallet balance', exact: true })
    await expect(summary).toContainText('391,054')
    await expect(summary).toContainText('20,000')
    await expect(summary).not.toContainText('$')
    expect((await summary.boundingBox())!.y).toBeLessThan(340)
    await expect(page.locator('.ob-wallet-connection')).not.toHaveAttribute('open', '')
    await expect(page.getByLabel('Lock after inactivity', { exact: true })).toHaveCount(0)
    expect(await page.locator('.ob-content').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await page.screenshot({ path: info.outputPath(`wallet-${width}.png`) })
    await page.getByRole('button', { name: 'Copy receive address', exact: true }).click()
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(address)
    await expect(page.locator('.ob-wallet-receive .ob-copy-button')).toHaveText('Copied')
    const filter = page.getByRole('combobox', { name: 'Filter wallet activity', exact: true })
    await filter.selectOption('out')
    await expect(page.locator('.ob-wallet-activity li')).toHaveCount(1)
    await page.locator('.ob-wallet-activity summary').click()
    await expect(page.locator('.ob-wallet-transaction-details')).toContainText('b'.repeat(64))
    await filter.selectOption('all')
    await expect(page.locator('.ob-wallet-activity li')).toHaveCount(3)
    await page.getByRole('button', { name: 'Receive bitcoin', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Receive bitcoin', exact: true })
    await expect(dialog.getByRole('img', { name: 'Bitcoin payment QR code', exact: true })).toHaveAttribute('data-payment-uri', `bitcoin:${address}`)
    expect(addressCalls).toBe(0)
    await dialog.getByRole('button', { name: 'Copy receive address', exact: true }).click()
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(address)
    const qrBox = await dialog.getByRole('img').boundingBox()
    expect(Math.abs(qrBox!.width - qrBox!.height)).toBeLessThan(1)
    await page.screenshot({ path: info.outputPath(`receive-${width}.png`) })
    await dialog.getByRole('button', { name: 'Done', exact: true }).click()
    await page.getByRole('button', { name: 'New address', exact: true }).click()
    await expect(page.locator('.ob-wallet-receive code')).toHaveText(freshAddress)
    expect(addressCalls).toBe(1)
    await page.getByRole('button', { name: 'Copy receive address', exact: true }).click()
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(freshAddress)
    await page.getByRole('button', { name: 'Wallet settings', exact: true }).click()
    const settings = page.getByRole('dialog', { name: 'Wallet settings', exact: true })
    await expect(settings.getByText('Lock after inactivity', { exact: true })).not.toBeVisible()
    await settings.locator('.ob-lock-details summary').click()
    await expect(settings.getByRole('combobox', { name: 'Lock after inactivity', exact: true })).toBeVisible()
    await page.screenshot({ path: info.outputPath(`settings-${width}.png`) })
  })
}

test('locked wallet keeps balances and receiving available without unlocking', async ({ page }) => {
  await fixture(page, 'mainnet', true)
  let unlocks = 0
  page.on('request', request => { if (request.url().endsWith('/unlock')) unlocks++ })
  await openWallet(page)
  await expect(page.locator('.ob-wallet-summary')).toContainText('391,054')
  await expect(page.locator('.ob-wallet-summary')).toContainText('Locked · receiving available')
  await page.getByRole('button', { name: 'Receive bitcoin', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Receive bitcoin', exact: true })).toBeVisible()
  expect(unlocks).toBe(0)
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Wallet settings', exact: true }).getByLabel('Wallet passphrase', { exact: true })).toBeVisible()
})

test('failed refresh preserves the last balance and warns that it is stale', async ({ page }) => {
  await fixture(page)
  await openWallet(page)
  await expect(page.locator('.ob-wallet-spendable')).toContainText('391,054')
  await page.route('**/wallet', route => route.fulfill({ status: 503, json: { error: 'Node temporarily unavailable.' } }))
  await page.getByRole('button', { name: 'Refresh wallet', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Showing the last available balance.')
  await expect(page.getByRole('heading', { name: 'Last known spendable balance', exact: true })).toBeVisible()
  await expect(page.locator('.ob-wallet-spendable')).toContainText('391,054')
  await expect(page.getByRole('button', { name: 'Receive bitcoin', exact: true })).toBeDisabled()
})

test('a stalled wallet request ends with a retryable error instead of loading forever', async ({ page }) => {
  await fixture(page)
  await page.route('**/wallet', () => { /* The node accepts the request but never answers. */ })
  await page.clock.install()
  await openWallet(page)
  await expect(page.getByText('Loading balance…', { exact: true })).toBeVisible()
  await page.clock.fastForward(13000)
  await expect(page.getByRole('alert')).toContainText('The wallet request timed out.')
  await expect(page.getByRole('button', { name: 'Refresh wallet', exact: true })).toBeEnabled()
  await expect(page.locator('.ob-wallet-spendable')).toContainText('Balance unavailable')
})
