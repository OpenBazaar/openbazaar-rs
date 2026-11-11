import { test, expect, type Page } from '@playwright/test'

const caseId = 'ab'.repeat(16)
const otherId = 'ef'.repeat(16)
const outpoint = `${'cd'.repeat(32)}:0`
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: true, rememberAvailable: true, backupConfirmed: false, minPassphraseChars: 8 }
const letters = [
  { id: caseId, favors: 'buyer', memo: 'The parcel has not arrived. Tracking has not changed for twelve days. I requested a refund after contacting the seller.', outpoint, receivedHeight: 160400, ruled: false },
  { id: caseId, favors: 'vendor', memo: 'The order was dispatched with tracking. I have supplied the postage receipt and the latest carrier update.', outpoint, receivedHeight: 160402, ruled: false },
  { id: otherId, favors: 'buyer', memo: 'The item was returned and the refund is outstanding.', outpoint: `${'12'.repeat(32)}:1`, receivedHeight: 160390, ruled: true }
]

async function desk(page: Page, initial = letters, state = 'unlocked', moderates = true) {
  let disputes = initial.map(item => ({ ...item }))
  let status = { ...lock, state }
  const rulings: unknown[] = []
  let reveals = 0
  await page.addInitScript(() => localStorage.setItem('ob-home-device', '01'.repeat(32)))
  // A signed-in moderator presentation fixture, never a real browser credential.
  await page.route('**/', async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replace('<head>', '<head><meta name="ob-home-node" content="true">') })
  })
  await page.route('**/remote/hello', route => route.fulfill({ json: { nodeId: '90'.repeat(32) } }))
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, moderates } }))
  await page.route('**/remote/devices', route => route.fulfill({ json: { devices: [
    { id: '11'.repeat(8), name: 'Desk browser', platform: 'browser', pairedAt: 1794315600, lastSeen: 1794316800, lastVia: 'web', pairedBy: null, thisDevice: true },
    { id: '22'.repeat(8), name: 'Operations laptop', platform: 'desktop', pairedAt: 1794229200, lastSeen: 1794315000, lastVia: 'tor', pairedBy: null, thisDevice: false }
  ] } }))
  await page.route('**/remote', route => route.fulfill({ json: { enabled: true, onion: `${'a'.repeat(56)}.onion`, devices: 2, webShop: true } }))
  await page.route('**/remote/pending', route => route.fulfill({ json: { pending: [] } }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: status }))
  await page.route('**/unlock', route => { status = { ...status, state: 'unlocked' }; return route.fulfill({ json: status }) })
  await page.route('**/moderator/disputes', route => route.fulfill({ json: { disputes } }))
  await page.route('**/moderator/disputes/*/rule', route => {
    const body = route.request().postDataJSON()
    rulings.push(body)
    const id = route.request().url().split('/').at(-2)
    disputes = disputes.map(letter => letter.id === id && letter.favors === body.favor ? { ...letter, ruled: true } : letter)
    return route.fulfill({ json: {} })
  })
  await page.route('**/wallet/phrase', route => { reveals++; return route.fulfill({ status: 403, json: { error: "The backup phrase is shown only on the node's own machine." } }) })
  await page.goto('/')
  if (moderates) await expect(page.getByRole('heading', { name: 'Moderator desk', exact: true })).toBeVisible()
  return { rulings, reveals: () => reveals }
}

for (const [width, dark] of [[320, false], [768, true], [1366, false], [1920, true]] as const) {
  test(`moderator review and access views fit ${width}px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 })
    await page.emulateMedia({ colorScheme: dark ? 'dark' : 'light' })
    await desk(page)
    await expect(page.locator('.ob-case-queue li')).toHaveCount(1)
    await expect(page.getByRole('region', { name: 'buyer statement', exact: true })).toBeVisible()
    await expect(page.getByRole('region', { name: 'seller statement', exact: true })).toBeVisible()
    await expect(page.getByLabel('Wallet passphrase', { exact: true })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Paired devices', exact: true })).toHaveCount(0)
    expect(await page.locator('.ob-desk').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await page.screenshot({ path: info.outputPath(`moderator-${width}.png`) })
    await page.getByRole('button', { name: 'Resolved 1', exact: true }).click()
    await expect(page.locator('.ob-case-detail')).toContainText('Ruled for the buyer')
    await page.getByRole('button', { name: 'Access & devices', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'Paired devices', exact: true })).toBeVisible()
    await expect(page.locator('.ob-device-list li')).toHaveCount(2)
    expect(await page.locator('.ob-desk').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await page.screenshot({ path: info.outputPath(`access-${width}.png`) })
    await page.getByRole('button', { name: 'Set up Tor Browser', exact: true }).click()
    const setup = page.getByRole('dialog', { name: 'Set up Tor Browser', exact: true })
    await expect(setup.getByLabel('Name this browser', { exact: true })).toBeVisible()
    await expect(page.getByRole('group', { name: 'Tor Browser credential', exact: true })).toHaveCount(0)
    await page.screenshot({ path: info.outputPath(`browser-setup-${width}.png`) })
    await setup.getByRole('button', { name: 'Cancel', exact: true }).click()
    if (width === 320) {
      await page.locator('.ob-moderator-mobile-menu > summary').click()
      await page.getByRole('button', { name: 'Wallet security', exact: true }).click()
      await expect(page.getByRole('dialog', { name: 'Wallet security', exact: true })).toBeVisible()
    }
  })
}

test('a ruling needs explicit review, cancellation sends nothing, and one ruling closes both letters', async ({ page }) => {
  const { rulings } = await desk(page)
  await page.getByRole('button', { name: 'Review ruling for buyer', exact: true }).click()
  const confirmation = page.getByRole('dialog', { name: 'Confirm final ruling', exact: true })
  await expect(confirmation).toContainText('cannot be changed')
  await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(rulings).toHaveLength(0)
  await page.getByRole('button', { name: 'Review ruling for seller', exact: true }).click()
  await confirmation.getByRole('button', { name: 'Confirm ruling for seller', exact: true }).click()
  await expect(confirmation).toHaveCount(0)
  expect(rulings).toEqual([{ favor: 'vendor' }])
  await expect(page.getByRole('heading', { name: 'No disputes awaiting review', exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Resolved 2', exact: true }).click()
  await expect(page.locator('.ob-case-detail')).toContainText('Ruled for the seller')
  await expect(page.locator('.ob-case-detail')).toContainText('Closed by the ruling for the other party')
})

test('locked operators can review but cannot issue a ruling', async ({ page }) => {
  const { rulings } = await desk(page, letters, 'locked')
  await expect(page.getByRole('button', { name: 'Review ruling for buyer', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  const security = page.getByRole('dialog', { name: 'Wallet security', exact: true })
  await expect(security).toContainText('sign a final escrow ruling')
  await expect(security.getByRole('button', { name: 'Unlock with this device', exact: true })).toHaveCount(0)
  await security.getByLabel('Wallet passphrase', { exact: true }).fill('disposable fixture passphrase')
  await security.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  await security.getByRole('button', { name: 'Done', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Review ruling for buyer', exact: true })).toBeEnabled()
  expect(rulings).toHaveLength(0)
})

test('remote backup refusal removes the unusable reveal form', async ({ page }) => {
  const state = await desk(page)
  await page.getByRole('button', { name: 'Wallet security', exact: true }).click()
  const security = page.getByRole('dialog', { name: 'Wallet security', exact: true })
  await security.getByLabel('Wallet passphrase', { exact: true }).fill('disposable fixture passphrase')
  await security.getByRole('button', { name: 'Show backup phrase', exact: true }).click()
  await expect(security.getByRole('heading', { name: 'Backup recovery is local only', exact: true })).toBeVisible()
  await expect(security.getByLabel('Wallet passphrase', { exact: true })).toHaveCount(0)
  await expect(security.getByRole('button', { name: 'Show backup phrase', exact: true })).toHaveCount(0)
  expect(state.reveals()).toBe(1)
})

test('empty and failed dispute responses have distinct states', async ({ page }, info) => {
  await desk(page, [])
  await expect(page.getByRole('heading', { name: 'No disputes awaiting review', exact: true })).toBeVisible()
  await page.screenshot({ path: info.outputPath('moderator-empty.png') })
  await page.route('**/moderator/disputes', route => route.fulfill({ status: 503, json: { error: 'Moderator mailbox unavailable.' } }))
  await page.getByRole('button', { name: 'Refresh disputes', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Moderator mailbox unavailable.')
  await expect(page.getByRole('heading', { name: 'Disputes unavailable', exact: true })).toBeVisible()
})

test('remote lock settings omit the local-only remembered-device option', async ({ page }) => {
  await desk(page)
  const settings: unknown[] = []
  await page.route('**/lock/settings', route => {
    settings.push(route.request().postDataJSON())
    return route.fulfill({ json: { ...lock, autoLockMinutes: 30 } })
  })
  await page.getByRole('button', { name: 'Wallet security', exact: true }).click()
  const security = page.getByRole('dialog', { name: 'Wallet security', exact: true })
  await security.locator('.ob-lock-details summary').click()
  const form = security.locator('.ob-lock-details form')
  await expect(form.getByRole('checkbox')).toHaveCount(0)
  await form.getByRole('combobox', { name: 'Lock after inactivity', exact: true }).selectOption('30')
  await form.getByLabel('Wallet passphrase', { exact: true }).fill('disposable fixture passphrase')
  await form.getByRole('button', { name: 'Save lock settings', exact: true }).click()
  await expect.poll(() => settings).toEqual([{ passphrase: 'disposable fixture passphrase', autoLockMinutes: 30 }])
})

test('the desk and the marketplace lead to each other', async ({ page }, info) => {
  let role: string | null = null
  await page.route('**/store/profile', route => route.fulfill({ json: { storeKey: '34'.repeat(32), role } }))
  await desk(page)
  const heading = page.getByRole('heading', { name: 'Moderator desk', exact: true })
  const link = page.getByRole('button', { name: 'Moderator desk', exact: true })
  const marketplace = page.getByRole('button', { name: 'Open marketplace', exact: true })
  // A node with no shop role yet shows onboarding, which must still lead back.
  await marketplace.click()
  await expect(page.getByRole('heading', { name: 'How will you use OpenBazaar?', exact: true })).toBeVisible()
  await page.screenshot({ path: info.outputPath('desk-link-onboarding.png') })
  await link.click()
  await expect(heading).toBeVisible()
  role = 'buyer'
  await marketplace.click()
  await expect(page.getByRole('navigation', { name: 'Moderation', exact: true })).toBeVisible()
  await page.screenshot({ path: info.outputPath('desk-link-sidebar.png') })
  await link.click()
  await expect(heading).toBeVisible()
})

test('a node that does not moderate opens the marketplace and offers no desk', async ({ page }) => {
  await page.route('**/store/profile', route => route.fulfill({ json: { storeKey: '34'.repeat(32), role: 'buyer' } }))
  await desk(page, letters, 'unlocked', false)
  await expect(page.getByRole('navigation', { name: 'Shopping', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Moderator desk', exact: true })).toHaveCount(0)
  await expect(page.getByRole('heading', { name: 'Moderator desk', exact: true })).toHaveCount(0)
})

test("this computer's own shop offers the desk when its node moderates", async ({ page }) => {
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, moderates: true } }))
  await page.route('**/moderator/disputes', route => route.fulfill({ json: { disputes: [] } }))
  await page.goto('/')
  await page.getByRole('button', { name: 'Moderator desk', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Moderator desk', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Sign out', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Open marketplace', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Shopping', exact: true })).toBeVisible()
})

test('a shop that still needs its pairing code offers the desk only once it is paired', async ({ page }) => {
  // e2e/server.mjs hands the page its disposable node's token; this page gets it only by pairing.
  let token = ''
  const pairs: unknown[] = []
  await page.route('**/', async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replace(/globalThis\.__OB_PAIR_TOKEN__="([0-9a-f]+)";/, (_, found) => { token = found; return '' }) })
  })
  await page.route('**/pair', route => { pairs.push(route.request().postDataJSON()); return route.fulfill({ json: pairs.length > 1 ? { token } : {} }) })
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, moderates: true } }))
  await page.goto('/')
  const pair = page.getByRole('button', { name: 'Enter pairing code', exact: true })
  await expect(pair).toBeVisible()
  await expect(page.getByRole('button', { name: 'Moderator desk', exact: true })).toHaveCount(0)
  // Asking whether the node moderates never asks the node for a pairing code.
  expect(pairs).toEqual([])
  await pair.click()
  await page.getByLabel('Pairing code', { exact: true }).fill('ABCD-EFGH')
  await page.getByRole('button', { name: 'Pair browser', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Moderator desk', exact: true })).toBeVisible()
  expect(pairs).toEqual([{}, { code: 'ABCD-EFGH' }])
})

test('a device that asks to pair is approved in the pairing dialog that covers the desk', async ({ page }) => {
  await desk(page)
  // Fixture values, never a real code.
  const code = { words: ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight'], link: `openbazaar://pair?v=2&s=${'a'.repeat(16)}&k=${'90'.repeat(32)}&o=${'a'.repeat(56)}`, expiresInSecs: 600 }
  const request = { requestId: '33'.repeat(16), name: 'Field phone', platform: 'ios', sas: ['🐶', '🐱', '🦊', '🐻'], sasNames: ['dog', 'cat', 'fox', 'bear'] }
  let waiting = false
  const decisions: unknown[] = []
  await page.route('**/remote/pairing', route => { waiting = true; return route.fulfill({ json: code }) })
  await page.route('**/remote/pending', route => route.fulfill({ json: { pending: waiting ? [request] : [] } }))
  await page.route('**/remote/pending/*', route => { decisions.push(route.request().postDataJSON()); waiting = false; return route.fulfill({ json: { device: null } }) })
  await page.getByRole('button', { name: 'Access & devices', exact: true }).click()
  await page.getByRole('button', { name: 'Pair a new device', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: 'Pair a new device', exact: true })
  await expect(dialog.getByRole('group', { name: 'New pairing code', exact: true })).toBeVisible()
  // The phone's request arrives while the modal dialog is open; the page polls for it.
  const asking = dialog.getByRole('group', { name: 'Field phone wants to pair', exact: true })
  await expect(asking).toContainText('dog')
  await expect(page.getByRole('group', { name: 'Field phone wants to pair', exact: true })).toHaveCount(1)
  await asking.getByRole('button', { name: 'Approve', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(decisions).toEqual([{ approve: true }])
})

// Fixture values, never a real code.
const pairingCode = { codeId: 'c0de'.repeat(3), words: ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight'], link: `openbazaar://pair?v=2&s=${'a'.repeat(16)}&k=${'90'.repeat(32)}&o=${'a'.repeat(56)}`, expiresInSecs: 600 }

test('a new pairing code is held until a phone can reach it, and shown with a warning when that takes too long', async ({ page }) => {
  await desk(page)
  let onion = 'publishing'
  await page.route('**/remote/pairing', route => { onion = 'publishing'; return route.fulfill({ json: { ...pairingCode, onion, lan: false } }) })
  // The node starts the code's ten minutes when it stops holding it, so the page shows that time and not the one from before the wait.
  await page.route('**/remote/pending', route => route.fulfill({ json: { pending: [], codes: [{ codeId: pairingCode.codeId, onion, expiresInSecs: 480 }] } }))
  await page.getByRole('button', { name: 'Access & devices', exact: true }).click()
  await page.getByRole('button', { name: 'Pair a new device', exact: true }).click()
  const offer = page.getByRole('dialog', { name: 'Pair a new device', exact: true }).getByRole('group', { name: 'New pairing code', exact: true })
  const qr = offer.getByRole('img', { name: 'Pairing code as a QR code', exact: true })
  await expect(offer.getByRole('heading')).toContainText('Publishing a private address for this code')
  await expect(offer.getByRole('status')).toContainText('The QR code and the eight words will show here as soon as it does')
  await expect(offer.getByRole('heading')).toContainText(/0:0[1-9]/)
  await expect(qr).toHaveCount(0)
  await expect(offer.locator('.ob-pair-words')).toHaveCount(0)
  await expect(offer.getByText('Pairing link', { exact: true })).toHaveCount(0)

  onion = 'ready'
  await expect(qr).toBeVisible()
  await expect(offer.getByRole('heading')).toContainText(/Pair within (8:00|7:5\d)/)
  await expect(offer.locator('.ob-pair-words li')).toHaveText(pairingCode.words)
  await expect(offer.getByRole('note')).toHaveCount(0)

  // The node gives up waiting after its limit: the code is shown, with what to expect.
  await offer.getByRole('button', { name: 'Done', exact: true }).click()
  await page.getByRole('button', { name: 'Pair a new device', exact: true }).click()
  await expect(offer.getByRole('heading')).toContainText('Publishing a private address for this code')
  await expect(qr).toHaveCount(0)
  onion = 'slow'
  await expect(qr).toBeVisible()
  await expect(offer.getByRole('note')).toContainText('taking unusually long to appear')
  await expect(offer.locator('.ob-pair-words li')).toHaveCount(8)
})

test('a pairing code that also works on the local network is shown at once', async ({ page }) => {
  await desk(page)
  await page.route('**/remote/pairing', route => route.fulfill({ json: { ...pairingCode, link: `${pairingCode.link}&l=192.168.1.20:8790`, onion: 'publishing', lan: true } }))
  await page.route('**/remote/pending', route => route.fulfill({ json: { pending: [], codes: [{ codeId: pairingCode.codeId, onion: 'publishing', expiresInSecs: 600 }] } }))
  await page.getByRole('button', { name: 'Access & devices', exact: true }).click()
  await page.getByRole('button', { name: 'Pair a new device', exact: true }).click()
  const offer = page.getByRole('dialog', { name: 'Pair a new device', exact: true }).getByRole('group', { name: 'New pairing code', exact: true })
  await expect(offer.getByRole('img', { name: 'Pairing code as a QR code', exact: true })).toBeVisible()
  await expect(offer.getByRole('heading')).toContainText(/Pair within (10:00|9:5\d)/)
  await expect(offer.getByRole('note')).toContainText('On the same network as your node this code works now')
})
