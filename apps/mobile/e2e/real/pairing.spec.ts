import { test, expect } from './harness.ts'
import { resolve } from 'node:path'

const DIST = resolve(__dirname, '../../dist')

test('a code made in Browsers settings pairs another client once', async ({ page, spawn, guard }) => {
  const node = await spawn('home', { role: 'buyer' })
  await node.setupWallet()
  await page.goto(node.ui)
  await page.getByRole('button', { name: 'Settings' }).click()
  await page.getByRole('button', { name: 'Browsers', exact: true }).click()
  await page.getByRole('button', { name: 'Create pairing code' }).click()
  const code = (await page.getByRole('status', { name: 'Pairing code' }).locator('code').first().textContent())?.trim()
  expect(code).toMatch(/\S{4,}/)

  const pair = (body: object) => fetch(`${node.address}/pair`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  const traded = await (await pair({ code })).json()
  expect(traded.token).toMatch(/^[0-9a-f]{64}$/i)
  const wallet = await fetch(`${node.address}/wallet`, { headers: { 'X-Ob-Pair': traded.token } })
  expect(wallet.status).toBe(200)
  const again = await (await pair({ code })).json()
  expect(again.token).toBeUndefined()
  guard.allowOpenDialog()
})

// The page the desktop app embeds: the node serves the shop itself (--local-shop),
// and a browser on this computer is signed in without a code.
test('the node’s own shop page opens the wallet on this computer without a code', async ({ browser, spawn }) => {
  const node = await spawn('desktop', { role: 'buyer', args: ['--local-shop', DIST] })
  await node.setupWallet()
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(`${node.address}/`)
  await page.getByRole('navigation', { name: 'Orders and wallet' }).getByRole('button', { name: 'Wallet' }).click()
  await expect(page.getByRole('region', { name: 'Wallet balance' })).toContainText('Unlocked · payments require passphrase')
  await context.close()
})
