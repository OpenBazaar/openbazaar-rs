import { test, expect, type Locator, type Page } from '@playwright/test'

// Dialogs are portaled out of the page; they must still get the app's styles and theme.
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: false, rememberAvailable: true, backupConfirmed: false, minPassphraseChars: 8 }
const wallet = { network: 'testnet4', balance: { spendableSats: 391054, confirmedSats: 391054, pendingSats: 0, totalSats: 391054 }, receiveAddress: `tb1p${'q'.repeat(58)}`, lastSync: 1794272400, syncError: null, activity: [] }

async function fixture(page: Page) {
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, publicChainServer: true, lightning: 'unavailable', silentPayments: 'code-only', proxy: 'socks5h://127.0.0.1:9050', chainBackend: { backend: 'compact-filters', privacy: 'public', scannedHeight: 160251, peers: 0, fallbackActive: true, reveals: 'Tip, fees and broadcast transactions.' } } }))
  await page.route('**/wallet', route => route.fulfill({ json: wallet }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: lock }))
  await page.route('**/orders', route => route.fulfill({ json: { orders: [] } }))
}

async function expectStyledButton(button: Locator) {
  const style = await button.evaluate(el => { const s = getComputedStyle(el); return { radius: parseFloat(s.borderTopLeftRadius), height: el.getBoundingClientRect().height, font: s.fontFamily } })
  expect(style.radius, 'app button radius').toBeGreaterThanOrEqual(4)
  expect(style.height, 'app button height').toBeGreaterThanOrEqual(40)
  expect(style.font).toContain('Instrument Sans')
}

for (const dark of [false, true]) {
  test(`wallet settings dialog keeps app styles (${dark ? 'dark' : 'light'})`, async ({ page }, info) => {
    await fixture(page)
    await page.goto('/')
    await page.getByRole('button', { name: 'Wallet', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Your wallet', exact: true })).toBeVisible()
    if (await page.getByRole('switch', { name: 'Dark mode', exact: true }).isChecked() !== dark) await page.getByRole('switch', { name: 'Dark mode', exact: true }).click()
    await page.getByRole('button', { name: 'Wallet settings', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: 'Wallet settings', exact: true })
    await expect(dialog).toBeVisible()
    await page.screenshot({ path: process.env.OB_SHOT && dark ? process.env.OB_SHOT : info.outputPath(`wallet-settings-${dark ? 'dark' : 'light'}.png`) })
    for (const name of ['Done', 'Lock now', 'Show backup phrase']) await expectStyledButton(dialog.getByRole('button', { name, exact: true }))
    const padding = await dialog.evaluate(el => parseFloat(getComputedStyle(el).paddingLeft))
    expect(padding, 'dialog card has padding').toBeGreaterThanOrEqual(16)
    expect(await dialog.evaluate(el => el.closest('.ob-app') !== null), 'dialog mounts inside the app style scope').toBe(true)
    const status = await dialog.getByText(/payments require passphrase/).boundingBox()
    const lockNow = await dialog.getByRole('button', { name: 'Lock now', exact: true }).boundingBox()
    expect(status!.x + status!.width <= lockNow!.x || status!.y + status!.height <= lockNow!.y || lockNow!.y + lockNow!.height <= status!.y, 'status does not collide with Lock now').toBeTruthy()
  })
}
