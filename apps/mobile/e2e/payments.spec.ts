import { test, expect, type Page } from '@playwright/test'

test.afterEach(async ({ page }) => {
  // Let any wallet refresh finish before Playwright disposes its API responses.
  await page.unrouteAll({ behavior: 'wait' })
})

async function checkout(page: Page) {
  await page.goto('/')
  await expect(
    page.getByText('Testnet4 · test coins, no value', { exact: true })
  ).toBeVisible()
  await page
    .getByLabel('Search listings', { exact: true })
    .fill('Field recordings')
  await page
    .getByRole('button', { name: 'View Field recordings', exact: true })
    .click()
  await page.getByRole('button', { name: 'Buy now', exact: true }).click()
  await page
    .getByLabel('Delivery note or reply contact', { exact: true })
    .fill('Deliver through this disposable test node.')
  const created = page.waitForResponse(
    r => r.url().endsWith('/checkout') && r.request().method() === 'POST'
  )
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  const response = await created
  expect(response.ok()).toBeTruthy()
  const receipt = await response.json()
  await expect(
    page.getByRole('heading', { name: 'Order created', exact: true })
  ).toBeVisible()
  await expect(
    page.getByText('Awaiting payment', { exact: true })
  ).toBeVisible()
  return { order: receipt.orders[0], id: receipt.orders[0].orderId as string }
}
async function fundedUi(page: Page, network = 'testnet4') {
  // Only supply the UI funding precondition: the disposable real node is unfunded.
  // Checkout and /pay hit that node. Its worker reports a real chain error against
  // the local failing Esplora stub. No funds or confirmations are fabricated there.
  await page.route('**/wallet', async route => {
    const response = await route.fetch()
    const wallet = await response.json()
    await route.fulfill({
      response,
      json: {
        ...wallet,
        network,
        syncError: null,
        balance: {
          ...wallet.balance,
          spendableSats: 1000000,
          confirmedSats: 1000000,
          totalSats: 1000000
        }
      }
    })
  })
}

const PASSPHRASE = 'e2e wallet passphrase'
const UNLOCKED = {
  state: 'unlocked',
  autoLockMinutes: 15,
  autoLockRange: [1, 60],
  lockInSecs: 900,
  failedAttempts: 0,
  retryAfterSecs: 0,
  rememberDevice: false,
  rememberAvailable: true,
  backupConfirmed: true,
  minPassphraseChars: 8
}
/** Set the passphrase on the real node (first run), or unlock it. */
async function protect(page: Page) {
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
  const setup = page.getByRole('heading', { name: 'Protect this wallet' })
  const locked = page.getByRole('heading', { name: 'Wallet locked' })
  const unlocked = page.getByRole('heading', { name: 'Wallet unlocked' })
  await expect(setup.or(locked).or(unlocked)).toBeVisible()
  if (await setup.isVisible()) {
    await page.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
    await page.getByLabel('Repeat passphrase', { exact: true }).fill(PASSPHRASE)
    await page.getByRole('button', { name: 'Set passphrase', exact: true }).click()
    await expect(page.locator('.ob-backup-phrase li')).toHaveCount(12)
    const words = await page.locator('.ob-backup-phrase li').allInnerTexts()
    await page.getByRole('button', { name: 'I wrote it down', exact: true }).click()
    for (const label of await page.locator('.ob-wallet-lock .ob-field span').allInnerTexts())
      await page
        .getByLabel(label, { exact: true })
        .fill(words[Number(label.replace('Word ', '')) - 1])
    await page.getByRole('button', { name: 'Confirm backup', exact: true }).click()
  } else if (await locked.isVisible()) {
    await page.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
    await page.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  }
  await expect(unlocked).toBeVisible()
}
/**
 * An unlocked wallet and a node that quotes and issues tickets. The node's
 * own lock, quote, and ticket checks are covered by the Rust tests.
 */
async function steppedUp(page: Page, network = 'testnet4') {
  const tickets: { quote: string; passphrase: string }[] = []
  const terms = (orderId: string) => ({
    orderId,
    network,
    amountSats: 4321,
    feeCapSats: 300,
    address: `${network === 'mainnet' ? 'bc1q' : 'tb1q'}quotedbythenode`,
    recipientScript: '0014',
    feeRateSatVb: 2,
    expiresInSecs: 60
  })
  for (const path of ['**/lock', '**/lock/activity'])
    await page.route(path, route => route.fulfill({ json: UNLOCKED }))
  await page.route('**/orders/*/pay/quote', route => {
    const id = decodeURIComponent(route.request().url().split('/').at(-3) ?? '')
    return route.fulfill({ json: { ...terms(id), quote: `q-${id}` } })
  })
  await page.route('**/spend-tickets', route => {
    const body = route.request().postDataJSON()
    tickets.push(body)
    const id = String(body.quote).slice(2)
    return route.fulfill({ json: { ...terms(id), ticket: `t-${id}` } })
  })
  return tickets
}
async function stepUp(page: Page, confirm = 'Confirm payment') {
  await page.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
  await page.getByRole('button', { name: confirm, exact: true }).click()
}

test('testnet4 wallet, setup, real checkout, and a real quote request', async ({
  page
}) => {
  await page.goto('/')
  await expect(
    page.getByText('Testnet4 · test coins, no value', { exact: true })
  ).toBeVisible()
  await protect(page)
  const receive = page.locator('.ob-wallet-receive code')
  await expect(receive).toHaveText(/^tb1/)
  const original = await receive.innerText()
  await expect(
    page.getByRole('img', { name: 'Bitcoin payment QR code' })
  ).toHaveAttribute('data-payment-uri', `bitcoin:${original}`)
  await page.getByRole('button', { name: 'New address', exact: true }).click()
  await expect(receive).not.toHaveText(original)
  await expect(
    page.getByRole('img', { name: 'Bitcoin payment QR code' })
  ).toHaveAttribute('data-payment-uri', `bitcoin:${await receive.innerText()}`)
  const { id } = await checkout(page)
  const pay = page.getByRole('button', {
    name: 'Pay from your node wallet',
    exact: true
  })
  await expect(pay).toBeDisabled()
  await fundedUi(page)
  await page
    .getByRole('button', { name: 'Fund your node wallet', exact: true })
    .click()
  await page
    .getByRole('button', { name: 'Refresh wallet', exact: true })
    .click()
  await expect(page.locator('.ob-wallet-amount strong').first()).toContainText(
    '1,000,000'
  )
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .first()
    .click()
  await expect(pay).toBeEnabled()
  const pays: string[] = []
  page.on('request', r => {
    if (r.url().endsWith(`/orders/${id}/pay`)) pays.push(r.url())
  })
  const quoted = page.waitForResponse(
    r =>
      r.url().endsWith(`/orders/${id}/pay/quote`) &&
      r.request().method() === 'POST'
  )
  await pay.click()
  // The disposable node's chain stub fails, so the real quote cannot sync.
  const response = await quoted
  expect(response.status()).toBe(502)
  expect(
    /^[a-f0-9]{64}$/i.test(response.request().headers()['x-ob-pair'] ?? '')
  ).toBeTruthy()
  await expect(page.getByRole('alert').first()).toBeVisible()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  expect(pays).toEqual([])
})

test('a locked wallet unlocks before the step-up', async ({ page }) => {
  const { id } = await checkout(page)
  await fundedUi(page)
  const tickets = await steppedUp(page)
  let state = 'locked'
  const unlocks: unknown[] = []
  // Registered after steppedUp, so these answer first.
  await page.route('**/lock', route =>
    route.fulfill({ json: { ...UNLOCKED, state } })
  )
  await page.route('**/unlock', route => {
    unlocks.push(route.request().postDataJSON())
    state = 'unlocked'
    return route.fulfill({ json: { ...UNLOCKED, state } })
  })
  await page.reload()
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .first()
    .click()
  await page
    .getByRole('button', { name: 'Pay from your node wallet', exact: true })
    .click()
  const unlock = page.getByRole('dialog', { name: 'Unlock wallet' })
  await expect(unlock).toBeVisible()
  await unlock.getByLabel('Wallet passphrase', { exact: true }).fill(PASSPHRASE)
  await unlock.getByRole('button', { name: 'Unlock wallet', exact: true }).click()
  expect(unlocks).toEqual([{ passphrase: PASSPHRASE }])
  const confirm = page.getByRole('dialog', { name: 'Confirm payment' })
  await expect(confirm).toBeVisible()
  await expect(confirm).toContainText('4,321 sats')
  await expect(confirm).toContainText('tb1qquotedbythenode')
  await expect(confirm).toContainText('At most 300 sats')
  await expect(confirm).toContainText(id)
  expect(tickets).toEqual([])
})

test('mainnet shows the node’s terms and needs the passphrase before confirm:true', async ({
  page
}) => {
  const { id, order } = await checkout(page)
  await fundedUi(page, 'mainnet')
  const tickets = await steppedUp(page, 'mainnet')
  await page.route('**/network', async route => {
    const response = await route.fetch()
    await route.fulfill({
      response,
      json: { ...(await response.json()), network: 'mainnet' }
    })
  })
  let state = {
    ...order,
    id,
    created: 1,
    network: 'mainnet',
    receivedSats: 0,
    confirmations: 0,
    txids: [],
    paying: false,
    payment: {
      ...order.payment,
      network: 'mainnet',
      address: 'bc1qtestmainnetdisplayonly'
    }
  }
  await page.route(`**/orders/${id}`, route => route.fulfill({ json: state }))
  await page.route('**/orders', route =>
    route.fulfill({ json: { orders: [state] } })
  )
  const bodies: unknown[] = []
  // This endpoint is intercepted: no mainnet transaction is attempted.
  await page.route(`**/orders/${id}/pay`, async route => {
    bodies.push(route.request().postDataJSON())
    state = { ...state, paying: true }
    await route.fulfill({ status: 202, json: state })
  })
  await page.reload()
  await expect(page.getByText('Mainnet', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .click()
  const pay = page.getByRole('button', {
    name: 'Pay from your node wallet',
    exact: true
  })
  await expect(pay).toBeEnabled()
  await pay.click()
  const dialog = page.getByRole('dialog', { name: 'Send real bitcoin?' })
  await expect(dialog).toBeVisible()
  // The node's quoted values, not the order the client already had.
  await expect(dialog).toContainText('bc1qquotedbythenode')
  await expect(dialog).toContainText('4,321 sats')
  expect(bodies).toEqual([])
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(bodies).toEqual([])
  expect(tickets).toEqual([])
  await pay.click()
  await stepUp(page, 'Confirm mainnet payment')
  await expect
    .poll(() => bodies)
    .toEqual([{ ticket: `t-${id}`, confirm: true }])
  expect(tickets).toEqual([{ quote: `q-${id}`, passphrase: PASSPHRASE }])
  await expect(pay).toBeDisabled()
})

test('partial payment QR, confirmations, tx links, and status polling', async ({
  page
}) => {
  const { id, order } = await checkout(page)
  await fundedUi(page)
  let state = {
    ...order,
    id,
    created: 1,
    network: 'testnet4',
    receivedSats: 1500,
    confirmations: 0,
    txids: ['a'.repeat(64)],
    paying: false
  }
  await page.route(`**/orders/${id}`, route => route.fulfill({ json: state }))
  await page.route('**/orders', route =>
    route.fulfill({ json: { orders: [state] } })
  )
  await page.clock.install()
  await page.reload()
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .click()
  const due = order.totalSats - 1500
  await expect(
    page.getByText(
      `Still owed: ${due.toLocaleString('en-US')} sats (${(due / 1e8).toFixed(8)} BTC)`,
      { exact: true }
    )
  ).toBeVisible()
  await page.locator('.ob-external-payment summary').click()
  await expect(
    page.getByRole('img', { name: 'Bitcoin payment QR code' })
  ).toHaveAttribute(
    'data-payment-uri',
    `bitcoin:${order.payment.address}?amount=${(due / 1e8).toFixed(8)}`
  )
  await expect(page.locator('.ob-transaction-link')).toHaveAttribute(
    'href',
    `https://mempool.space/testnet4/tx/${'a'.repeat(64)}`
  )
  state = { ...state, receivedSats: order.totalSats, status: 'Payment seen' }
  await page.clock.fastForward(13000)
  await expect(page.locator('.ob-payment-progress strong')).toHaveText(
    'Payment seen'
  )
  state = { ...state, status: 'Paid', confirmations: 2 }
  await page.clock.fastForward(13000)
  await expect(page.locator('.ob-payment-progress strong')).toHaveText('Paid')
  await expect(page.getByText('Confirmed', { exact: true })).toBeVisible()
  await expect(page.getByText(/Waiting for/)).toHaveCount(0)
  await expect(
    page.getByRole('button', { name: 'Pay from your node wallet', exact: true })
  ).toHaveCount(0)
})

test('network mismatch and escrow are read-only', async ({ page }) => {
  const { id, order } = await checkout(page)
  await fundedUi(page)
  let state = {
    ...order,
    id,
    created: 1,
    network: 'mainnet',
    receivedSats: 0,
    confirmations: 0,
    txids: [],
    paying: false
  }
  await page.route(`**/orders/${id}`, route => route.fulfill({ json: state }))
  await page.route('**/orders', route =>
    route.fulfill({ json: { orders: [state] } })
  )
  await page.clock.install()
  await page.reload()
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .click()
  await expect(
    page.getByRole('button', { name: 'Pay from your node wallet', exact: true })
  ).toBeDisabled()
  await expect(
    page.getByText('Order, wallet, and node networks must match before paying.')
  ).toBeVisible()
  state = { ...state, settlement: 'escrow', status: 'Awaiting vendor terms' }
  await page.clock.fastForward(13000)
  await expect(
    page.getByText(
      'Escrow payment is not available in this client. This order’s status is read-only.'
    )
  ).toBeVisible()
  await expect(
    page.getByRole('button', { name: 'Pay from your node wallet', exact: true })
  ).toHaveCount(0)
})

test('a multi-seller checkout submits one separate pay request per seller', async ({
  page
}) => {
  await fundedUi(page)
  const tickets = await steppedUp(page)
  const bodies: Record<string, unknown> = {}
  await page.route('**/orders/*/pay', route => {
    const id = decodeURIComponent(route.request().url().split('/').at(-2) ?? '')
    bodies[id] = route.request().postDataJSON()
    return route.fulfill({ status: 202, json: { error: 'unused' } })
  })
  await page.goto('/')
  for (const title of ['Field recordings', 'Weeknight recipes']) {
    await page.getByLabel('Search listings', { exact: true }).fill(title)
    await page
      .getByRole('button', { name: `View ${title}`, exact: true })
      .click()
    await page.getByRole('button', { name: 'Add to bag', exact: true }).click()
  }
  await page.getByRole('button', { name: 'Your bag', exact: true }).click()
  await page
    .getByRole('button', { name: 'Continue to checkout', exact: true })
    .click()
  await page
    .getByLabel('Delivery note or reply contact', { exact: true })
    .fill('Digital test delivery.')
  const created = page.waitForResponse(
    r => r.url().endsWith('/checkout') && r.request().method() === 'POST'
  )
  await page.getByRole('button', { name: 'Place order', exact: true }).click()
  const receipt = await (await created).json()
  expect(receipt.orders).toHaveLength(2)
  const panels = page.locator('.ob-seller-receipt')
  await expect(panels).toHaveCount(2)
  for (let i = 0; i < 2; i++) {
    const button = panels
      .nth(i)
      .getByRole('button', { name: 'Pay from your node wallet', exact: true })
    await expect(button).toBeEnabled()
    const id = receipt.orders[i].orderId
    await button.click()
    await stepUp(page)
    await expect.poll(() => bodies[id]).toEqual({ ticket: `t-${id}` })
  }
  expect(tickets.map(t => t.quote)).toEqual(
    receipt.orders.map((o: { orderId: string }) => `q-${o.orderId}`)
  )
})

test('pending payments disable spending and node failures remain visible', async ({
  page
}) => {
  const { id, order } = await checkout(page)
  await fundedUi(page)
  await steppedUp(page)
  let state = {
    ...order,
    id,
    created: 1,
    network: 'testnet4',
    receivedSats: 0,
    confirmations: 0,
    txids: [],
    paying: true,
    payError: null as string | null
  }
  await page.route(`**/orders/${id}`, route => route.fulfill({ json: state }))
  await page.route('**/orders', route =>
    route.fulfill({ json: { orders: [state] } })
  )
  await page.clock.install()
  await page.reload()
  await page.getByRole('button', { name: 'Your orders', exact: true }).click()
  // The first order fetch must finish before the clock jumps, or the poll joins it.
  const loaded = page.waitForResponse(r => r.url().endsWith(`/orders/${id}`))
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .click()
  await loaded
  const pay = page.getByRole('button', {
    name: 'Pay from your node wallet',
    exact: true
  })
  await expect(pay).toBeDisabled()
  state = {
    ...state,
    paying: false,
    payError: 'Insufficient funds including the network fee.'
  }
  await page.clock.fastForward(13000)
  await expect(page.getByRole('alert')).toContainText(
    'Insufficient funds including the network fee.'
  )
  await expect(pay).toBeEnabled()
  let status = 409
  await page.route(`**/orders/${id}/pay`, route =>
    route.fulfill({
      status,
      json: {
        error:
          status === 409
            ? 'This order is already being paid.'
            : 'The order address belongs to another network.'
      }
    })
  )
  await pay.click()
  await stepUp(page)
  await expect(page.getByRole('alert')).toContainText(
    'This order is already being paid.'
  )
  status = 400
  await expect(pay).toBeEnabled()
  await pay.click()
  await stepUp(page)
  await expect(page.getByRole('alert')).toContainText(
    'The order address belongs to another network.'
  )
})
