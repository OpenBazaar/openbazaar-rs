import { test, expect, type Page, type TestInfo } from '@playwright/test'

// Moderator identity in the three places it shows: the desk's profile, the
// store's accepted list, and the checkout picker. The node's answers are
// mocked; the Rust harness covers the node.
const builtInId = 'f9f279d91e97d7e2273ac15a73caa2646422d0d51824075c259b2084eb98e5ff'
const inesId = '8f4f09e776d5fe8e5f807ac7c7008fbf42701d88cef9a8b4795a654db9b197b8'
const address = `http://${'b'.repeat(56)}.onion`
const builtIn = { id: builtInId, name: 'June Pell', description: 'The built-in test moderator. Its keys are public, so it is for testnet4 coins only.', terms: '', languages: ['en'], feeSats: 1000, address: `http://${'c'.repeat(56)}.onion`, builtIn: true, accepted: true, trackRecord: null }
const ines = { id: inesId, name: 'Ines Varga', description: 'Shipping and condition disputes. I answer within two days.', terms: 'Send tracking, photos of the parcel as it arrived, and your messages with the other party.', languages: ['en', 'pt-br'], feeSats: 1500, address, builtIn: false, accepted: true, trackRecord: { rulings: 3, stores: 2, checkedAt: 1794316800 } }
const lock = { state: 'unlocked', autoLockMinutes: 15, autoLockRange: [1, 60], lockInSecs: 900, failedAttempts: 0, retryAfterSecs: 0, rememberDevice: true, rememberAvailable: true, backupConfirmed: true, minPassphraseChars: 8 }
// The narrow viewport is tall so one screenshot holds a whole section.
const widths = [[1366, 900], [390, 1800]] as const

// OB_SHOT_DIR keeps the screenshots where a reviewer can open them.
const shot = (info: TestInfo, name: string) => process.env.OB_SHOT_DIR ? `${process.env.OB_SHOT_DIR}/${name}` : info.outputPath(name)

test.afterEach(async ({ page }) => {
  await page.unrouteAll({ behavior: 'wait' })
})

async function desk(page: Page, state = 'unlocked') {
  let profile: Record<string, unknown> = { id: inesId, name: '', description: '', terms: '', languages: [], feeSats: 1000, minFeeSats: 330, published: false, publishedAddress: null, address, trackRecord: { rulings: 0, stores: 0 }, builtIn: { id: builtInId, name: 'June Pell' } }
  const edits: unknown[] = []
  await page.addInitScript(() => localStorage.setItem('ob-home-device', '01'.repeat(32)))
  await page.route('**/', async route => {
    const response = await route.fetch()
    await route.fulfill({ response, body: (await response.text()).replace('<head>', '<head><meta name="ob-home-node" content="true">') })
  })
  await page.route('**/remote/hello', route => route.fulfill({ json: { nodeId: '90'.repeat(32) } }))
  // The page signs in by asking for its devices.
  await page.route('**/remote/devices', route => route.fulfill({ json: { devices: [] } }))
  await page.route('**/network', route => route.fulfill({ json: { network: 'testnet4', confirmations: 1, moderates: true } }))
  for (const path of ['**/lock', '**/lock/activity']) await page.route(path, route => route.fulfill({ json: { ...lock, state } }))
  await page.route('**/moderator/disputes', route => route.fulfill({ json: { disputes: [] } }))
  await page.route('**/moderator/profile', route => {
    if (route.request().method() !== 'PUT') return route.fulfill({ json: profile })
    const edit = route.request().postDataJSON()
    edits.push(edit)
    profile = { ...profile, ...edit, published: true, publishedAddress: address }
    return route.fulfill({ json: profile })
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Profile', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Your moderator identity', exact: true })).toBeVisible()
  return edits
}

for (const [width, height] of widths) {
  test(`the owner names the moderator on the desk at ${width}px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height })
    const edits = await desk(page)
    const profile = page.getByRole('region', { name: 'Moderator profile', exact: true })
    await expect(profile).toContainText(inesId)
    await expect(profile).toContainText('Not published. Sellers cannot add this moderator until you save a name.')
    await expect(profile).toContainText('Works while the wallet is locked')
    await expect(profile).toContainText('Needs the wallet unlocked')
    const publish = profile.getByRole('button', { name: 'Sign and publish profile', exact: true })
    await expect(publish).toBeDisabled()
    await profile.getByLabel('Display name', { exact: true }).fill('Ines Varga')
    await profile.getByLabel('Fee per ruling (sats)', { exact: true }).fill('100')
    await expect(profile.getByRole('alert')).toContainText('at least 330 sats')
    await profile.getByLabel('Fee per ruling (sats)', { exact: true }).fill('1500')
    await profile.getByLabel('Short description', { exact: true }).fill(ines.description)
    await profile.getByLabel('Languages', { exact: true }).fill('en, pt-br')
    await profile.getByLabel('Terms and policy', { exact: true }).fill(ines.terms)
    await publish.click()
    await expect(profile.getByText('Profile signed and published.', { exact: true })).toBeVisible()
    expect(edits).toEqual([{ name: 'Ines Varga', description: ines.description, terms: ines.terms, languages: ['en', 'pt-br'], feeSats: 1500 }])
    await expect(profile).toContainText('Published. No public rulings yet.')
    await expect(profile.getByRole('button', { name: 'Sign and publish changes', exact: true })).toBeEnabled()
    expect(await page.locator('.ob-desk').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await page.screenshot({ path: shot(info, `ob-mod-desk-profile-${width}.png`), fullPage: true })
  })
}

test('a locked desk shows the profile and says what needs the wallet', async ({ page }) => {
  const edits = await desk(page, 'locked')
  const profile = page.getByRole('region', { name: 'Moderator profile', exact: true })
  await profile.getByLabel('Display name', { exact: true }).fill('Ines Varga')
  await expect(profile.getByRole('button', { name: 'Sign and publish profile', exact: true })).toBeDisabled()
  await expect(profile).toContainText('The wallet is locked.')
  await profile.getByRole('button', { name: 'Unlock it', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Wallet security', exact: true })).toBeVisible()
  expect(edits).toEqual([])
})

async function openSettings(page: Page, name: string) {
  const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name, exact: true }).click()
}

for (const [width, height] of widths) {
  test(`a seller adds and removes the moderators the store accepts at ${width}px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height })
    let list = [builtIn]
    const calls: unknown[] = []
    await page.route('**/store/profile', route => route.fulfill({ json: { storeKey: '34'.repeat(32), role: 'seller', name: 'Brook and Pine', region: 'Portland', setUp: true, published: true, network: 'testnet4', escrowAvailable: true, shipsTo: ['US'] } }))
    await page.route('**/moderators', route => {
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON()
        calls.push(body)
        list = body.address ? [builtIn, ines] : list.map(moderator => moderator.id === body.id ? { ...moderator, accepted: true } : moderator)
      }
      return route.fulfill({ json: { moderators: list, escrow: true } })
    })
    await page.route('**/moderators/*', route => {
      const id = route.request().url().split('/').at(-1)
      calls.push({ removed: id })
      list = list.map(moderator => moderator.id === id ? { ...moderator, accepted: false } : moderator)
      return route.fulfill({ json: { moderators: list, escrow: true } })
    })
    await page.goto('/')
    await openSettings(page, 'Store settings')
    const section = page.getByRole('region', { name: 'Moderators', exact: true })
    const accepted = section.getByRole('list', { name: 'Accepted moderators', exact: true })
    await expect(accepted.getByRole('listitem')).toHaveCount(1)
    await expect(accepted).toContainText('June Pell')
    await expect(accepted).toContainText('Built-in test moderator')
    await expect(accepted).toContainText('Test moderator with public keys. No track record.')

    await section.getByLabel('Add a moderator by address', { exact: true }).fill(` ${address} `)
    await section.getByRole('button', { name: 'Add moderator', exact: true }).click()
    await expect(accepted.getByRole('listitem')).toHaveCount(2)
    const added = accepted.getByRole('listitem').filter({ hasText: 'Ines Varga' })
    await expect(added).toContainText('Fee 1,500 sats if a dispute is ruled')
    await expect(added).toContainText('3 signed public rulings across 2 stores.')
    await expect(added).toContainText('8f4f09e7…b197b8')
    await expect(section.getByText('Moderator added. Your store record was signed again.', { exact: true })).toBeVisible()
    expect(await page.locator('.ob-content').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await section.scrollIntoViewIfNeeded()
    await section.screenshot({ path: shot(info, `ob-mod-store-settings-${width}.png`) })

    await section.getByRole('button', { name: 'Remove June Pell', exact: true }).click()
    await expect(accepted.getByRole('listitem')).toHaveCount(1)
    const known = section.getByRole('list', { name: 'Known moderators', exact: true })
    await expect(known).toContainText('June Pell')
    await section.getByRole('button', { name: 'Remove Ines Varga', exact: true }).click()
    await expect(section.getByText('You accept no moderators, so your store does not offer moderated escrow.', { exact: true })).toBeVisible()
    await section.getByRole('button', { name: 'Accept Ines Varga', exact: true }).click()
    await expect(accepted).toContainText('Ines Varga')
    expect(calls).toEqual([{ address }, { removed: builtInId }, { removed: inesId }, { id: inesId }])
  })
}

for (const [width, height] of widths) {
  test(`a buyer picks one of the store's moderators at checkout at ${width}px`, async ({ page }, info) => {
    await page.setViewportSize({ width, height })
    const asked: string[] = []
    const placed: { packages: Record<string, unknown>[] }[] = []
    await page.route('**/stores/*/moderators', route => {
      asked.push(route.request().url().split('/').at(-2)!)
      return route.fulfill({ json: { moderators: [builtIn, ines], unknown: 0 } })
    })
    await page.route('**/checkout', route => {
      placed.push(route.request().postDataJSON())
      return route.fulfill({ status: 400, json: { error: "The seller's node does not accept that moderator. Choose another one." } })
    })
    await page.goto('/')
    const opener = page.getByRole('button', { name: 'Open navigation', exact: true })
    if (await opener.isVisible()) await opener.click()
    await page.getByRole('button', { name: 'Products', exact: true }).click()
    await page.getByLabel('Search stores and listings', { exact: true }).fill('Field recordings')
    await page.getByRole('button', { name: 'View Field recordings', exact: true }).click()
    await page.getByRole('button', { name: 'Buy now', exact: true }).click()
    await page.getByLabel('Delivery note or reply contact', { exact: true }).fill('Deliver through this disposable test node.')
    await expect(page.getByRole('group', { name: 'Choose a moderator for Marlow Goods', exact: true })).toHaveCount(0)
    await page.getByRole('radio', { name: /^Moderated escrow/ }).check()
    const choice = page.getByRole('group', { name: 'Choose a moderator for Marlow Goods', exact: true })
    await expect(choice.getByRole('radio')).toHaveCount(2)
    await expect(choice.getByRole('radio', { name: /June Pell/ })).toBeChecked()
    await expect(choice).toContainText('Fee 1,000 sats if a dispute is ruled')
    await expect(choice).toContainText('3 signed public rulings across 2 stores.')
    await choice.getByRole('radio', { name: /Ines Varga/ }).check()
    expect(await page.locator('.ob-content').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBeTruthy()
    await choice.scrollIntoViewIfNeeded()
    await page.screenshot({ path: shot(info, `ob-mod-checkout-${width}.png`) })

    await page.getByRole('button', { name: 'Place order', exact: true }).click()
    await expect(page.getByRole('alert').filter({ hasText: "The seller's node does not accept that moderator." })).toBeVisible()
    // The picker asks for the moderators of the store being bought from.
    expect(asked[0]).toMatch(/^2b489bf1[0-9a-f]{56}$/)
    expect(placed).toHaveLength(1)
    expect(placed[0].packages).toHaveLength(1)
    expect(placed[0].packages[0]).toMatchObject({ settlement: 'escrow', moderator: inesId })

    // Paying the seller directly names no moderator.
    await page.getByRole('radio', { name: /^Pay the seller directly/ }).check()
    await expect(choice).toHaveCount(0)
    await page.getByRole('button', { name: 'Place order', exact: true }).click()
    await expect.poll(() => placed.length).toBe(2)
    expect(placed[1].packages[0]).not.toHaveProperty('moderator')
  })
}
