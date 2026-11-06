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

test('testnet4 wallet, real checkout, and a real per-order pay request', async ({
  page
}) => {
  await page.goto('/')
  await expect(
    page.getByText('Testnet4 · test coins, no value', { exact: true })
  ).toBeVisible()
  await page.getByRole('button', { name: 'Wallet', exact: true }).click()
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
  const paid = page.waitForResponse(
    r =>
      r.url().endsWith(`/orders/${id}/pay`) && r.request().method() === 'POST'
  )
  await pay.click()
  const response = await paid
  expect(response.status()).toBe(202)
  expect(response.request().postDataJSON()).toEqual({})
  expect(
    /^[a-f0-9]{64}$/i.test(response.request().headers()['x-ob-pair'] ?? '')
  ).toBeTruthy()
})

test('mainnet requires a confirmation dialog before confirm:true', async ({
  page
}) => {
  const { id, order } = await checkout(page)
  await fundedUi(page, 'mainnet')
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
  await expect(
    page.getByRole('dialog', { name: 'Send real bitcoin?' })
  ).toBeVisible()
  expect(bodies).toEqual([])
  await page.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(bodies).toEqual([])
  await pay.click()
  await page
    .getByRole('button', { name: 'Confirm mainnet payment', exact: true })
    .click()
  await expect.poll(() => bodies).toEqual([{ confirm: true }])
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
  await expect(
    page.getByText('Confirmations: 2 / 1', { exact: true })
  ).toBeVisible()
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
    const response = page.waitForResponse(
      r =>
        r.url().endsWith(`/orders/${receipt.orders[i].orderId}/pay`) &&
        r.request().method() === 'POST'
    )
    await button.click()
    const result = await response
    expect(result.status()).toBe(202)
    expect(result.request().postDataJSON()).toEqual({})
  }
})

test('pending payments disable spending and node failures remain visible', async ({
  page
}) => {
  const { id, order } = await checkout(page)
  await fundedUi(page)
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
  await page
    .getByRole('button', { name: 'Order from Marlow Goods', exact: true })
    .click()
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
  await expect(page.getByRole('alert')).toContainText(
    'This order is already being paid.'
  )
  status = 400
  await expect(pay).toBeEnabled()
  await pay.click()
  await expect(page.getByRole('alert')).toContainText(
    'The order address belongs to another network.'
  )
})
