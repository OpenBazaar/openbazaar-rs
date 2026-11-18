// Fixtures for tests against real disposable nodes. Nothing here mocks the
// node: `page.route` is refused, and every check runs against what the node
// actually answered.
import { test as base, expect, type Page, type TestInfo } from '@playwright/test'
import { createServer as createNetServer } from 'node:net'
import type { Server } from 'node:http'
import { join } from 'node:path'
import { mkdir } from 'node:fs/promises'
import { deflateSync } from 'node:zlib'
import { CONSOLE_ALLOWLIST } from './allowlist.ts'

export const PASSPHRASE = 'real node e2e passphrase'
// A valid 1×1 PNG; the node only keeps real images.
export const PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
  'base64'
)
export const image = (name: string) => ({ name, mimeType: 'image/png', buffer: PNG })

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})
function crc32(bytes: Buffer) {
  let c = 0xffffffff
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}
function chunk(type: string, data: Buffer) {
  const head = Buffer.alloc(8)
  head.writeUInt32BE(data.length)
  head.write(type, 4, 'ascii')
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])))
  return Buffer.concat([head, data, crc])
}
/** A distinct valid 1×1 RGB PNG for each `n`, so the node keeps each one. */
export function pngOf(n: number): Buffer {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(1, 0)
  ihdr.writeUInt32BE(1, 4)
  ihdr[8] = 8
  ihdr[9] = 2
  const pixel = Buffer.from([0, n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff])
  return Buffer.concat([Buffer.from('89504e470d0a1a0a', 'hex'), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(pixel)), chunk('IEND', Buffer.alloc(0))])
}
let pictures = 0
/** `count` distinct photos for a file input. */
export const photos = (count: number, prefix = 'photo') =>
  Array.from({ length: count }, (_, i) => ({ name: `${prefix}-${i + 1}.png`, mimeType: 'image/png', buffer: pngOf(++pictures * 7919) }))

export type ListingSeed = {
  title: string
  price?: number
  currency?: 'SAT' | 'USD'
  stock?: number
  images?: string[]
  kind?: 'physical' | 'digital'
}

export class TestNode {
  constructor(
    readonly name: string,
    readonly ui: string,
    readonly port: number,
    readonly data: string,
    readonly token: string,
    readonly logFile: string,
    /** The same shop with no pairing token handed to the page, when spawned with `bare`. */
    readonly bareUi: string | null = null
  ) {}

  /** The address another node on this machine follows this store at. */
  get address() {
    return `http://127.0.0.1:${this.port}`
  }

  async api<T = any>(method: string, path: string, body?: unknown, contentType = 'application/json'): Promise<T> {
    const r = await fetch(`${this.address}${path}`, {
      method,
      headers: { 'X-Ob-Pair': this.token, ...(body === undefined ? {} : { 'Content-Type': contentType }) },
      body: body === undefined ? undefined : contentType === 'application/json' ? JSON.stringify(body) : (body as BodyInit)
    })
    const text = await r.text()
    if (!r.ok) throw new Error(`${this.name}: ${method} ${path} answered ${r.status}: ${text}`)
    return (text ? JSON.parse(text) : null) as T
  }

  /** Sets the passphrase and confirms the backup, as onboarding does. Leaves the wallet unlocked. */
  async setupWallet(passphrase = PASSPHRASE) {
    const created = await this.api<{ phrase: string; confirmWords?: number[] }>('POST', '/setup', { passphrase })
    const words = created.phrase.split(' ')
    if (created.confirmWords?.length)
      await this.api('POST', '/setup/confirm', { words: created.confirmWords.map(n => words[n - 1]) })
    return words
  }

  lock() {
    return this.api<{ state: string }>('POST', '/lock')
  }

  unlock(passphrase = PASSPHRASE) {
    return this.api<{ state: string }>('POST', '/unlock', { passphrase })
  }

  lockState() {
    return this.api<{ state: string; autoLockMinutes: number }>('GET', '/lock')
  }

  async openStore(name: string) {
    await this.api('PUT', '/store/role', { role: 'seller' })
    return this.api('PUT', '/store/profile', { name, region: 'Portland', shipsTo: ['US'], processingTime: 'Ships in 2 days' })
  }

  async uploadImage(bytes: Buffer = pngOf(++pictures * 7919)) {
    const r = await this.api<{ hash: string }>('POST', '/media', bytes, 'image/png')
    return r.hash
  }

  async addListing(seed: ListingSeed) {
    await this.api('POST', '/store/listings', {
      title: seed.title,
      description: `${seed.title}, made for the end-to-end tests.`,
      category: 'Home',
      kind: seed.kind ?? 'physical',
      shipFrom: 'US',
      stock: seed.stock ?? 5,
      images: seed.images ?? [],
      currency: seed.currency ?? 'SAT',
      price: seed.price ?? 20000,
      shipping: 0,
      contentRating: 'general',
      contentTags: []
    })
  }

  follow(store: TestNode) {
    return this.api('POST', '/follows', { url: store.address })
  }
}

/** Onboarding's wallet step in the UI: passphrase, backup phrase, and its confirmation. */
export async function protectWallet(page: Page, passphrase = PASSPHRASE) {
  await expect(page.getByRole('heading', { name: 'Protect this wallet' })).toBeVisible()
  await page.getByLabel('Wallet passphrase', { exact: true }).fill(passphrase)
  await page.getByLabel('Repeat passphrase', { exact: true }).fill(passphrase)
  await page.getByRole('button', { name: 'Set passphrase' }).click()
  await expect(page.getByRole('heading', { name: 'Write down your backup phrase' })).toBeVisible()
  const words = await page.locator('.ob-backup-phrase li').allTextContents()
  await page.getByRole('button', { name: 'I wrote it down' }).click()
  const confirm = page.getByRole('heading', { name: 'Confirm your backup' })
  if (await confirm.isVisible()) {
    for (const label of await page.locator('.ob-wallet-lock .ob-field span').allTextContents()) {
      const position = Number(label.replace('Word ', ''))
      await page.getByLabel(label, { exact: true }).fill(words[position - 1])
    }
    await page.getByRole('button', { name: 'Confirm backup' }).click()
  }
  return words
}

type Spawn = (name: string, options?: { role?: 'buyer' | 'seller' | null; args?: string[]; bare?: boolean }) => Promise<TestNode>

export type Guard = {
  /** The node may answer `status` on paths matching `path` in this test. */
  allowStatus(status: number, path: RegExp): void
  /** A dialog may still be open when the test ends. */
  allowOpenDialog(): void
  /** Problems seen so far. */
  problems(): string[]
}

// nodes.mjs is a plain ES module shared with server.mjs; tests compile to CommonJS.
const nodes = import('../nodes.mjs') as Promise<any>
// Same as nodes.mjs: paths the UI server forwards to the node.
const NODE_ROUTES =
  /^\/(catalog|rates|network|wallet|pair|orders|checkout|addresses|labelers|content-labels|vendor-labels|listings|directory|follow|lock|unlock|setup|spend-tickets|stores|media|store|sales|follows|home|remote|ln|offers|coupons|xmr|zec)(\/|$)/

// Fixed exchange rates and no seed introductions: no outside network calls.
const DETERMINISTIC_ARGS = ['--fixed-rates', 'BTCUSD=100000,ZECUSD=50', '--no-introductions']

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createNetServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as { port: number }
      server.close(() => resolve(port))
    })
  })
}

async function attachLogs(info: TestInfo, nodes: TestNode[]) {
  for (const node of nodes) await info.attach(`node-${node.name}.log`, { path: node.logFile, contentType: 'text/plain' }).catch(() => {})
}

export const test = base.extend<{ spawn: Spawn; guard: Guard; seller: TestNode; buyer: TestNode }>({
  spawn: async ({}, use, info) => {
    const started: { node: TestNode; stop: () => Promise<void>; servers: Server[] }[] = []
    await use(async (name, options = {}) => {
      const [port, uiPort] = [await freePort(), await freePort()]
      const logFile = join(info.outputDir, `node-${name}.log`)
      await mkdir(info.outputDir, { recursive: true })
      const { pairToken, startNode, startUi, waitForNode } = await nodes
      const node = await startNode({ port, uiPort, role: options.role ?? null, logFile, extraArgs: [...DETERMINISTIC_ARGS, ...(options.args ?? [])], chainUrl: process.env.OB_E2E_CHAIN_URL })
      const server = startUi({ uiPort, nodePort: port, data: node.data })
      await waitForNode(node)
      const token = await pairToken(node.data)
      if (!token) throw new Error(`${name}: no pairing token in ${node.data}`)
      const servers = [server]
      let bareUi: string | null = null
      if (options.bare) {
        const barePort = await freePort()
        servers.push(startUi({ uiPort: barePort, nodePort: port, data: join(node.data, 'no-token') }))
        bareUi = `http://127.0.0.1:${barePort}`
      }
      const handle = new TestNode(name, `http://127.0.0.1:${uiPort}`, port, node.data, token, logFile, bareUi)
      started.push({ node: handle, stop: () => node.stop(), servers })
      return handle
    })
    if (info.status !== info.expectedStatus) await attachLogs(info, started.map(s => s.node))
    for (const server of started.flatMap(s => s.servers)) {
      server.closeAllConnections()
      server.close()
    }
    await Promise.all(started.map(s => s.stop()))
  },

  seller: async ({ spawn }, use) => {
    const node = await spawn('seller')
    await node.setupWallet()
    await node.openStore('Brook and Pine')
    await use(node)
  },

  buyer: async ({ spawn }, use) => {
    const node = await spawn('buyer', { role: 'buyer' })
    await node.setupWallet()
    await use(node)
  },

  guard: [
    // Depends on spawn so its checks run before the nodes stop.
    async ({ page, spawn: _spawn }, use, info) => {
      const refuse = () => {
        throw new Error('Real-node tests must not mock the node. Put mocked checks in the mocked project.')
      }
      page.route = refuse as Page['route']
      page.context().route = refuse as never
      const problems: string[] = []
      const allowed: { status: number; path: RegExp }[] = []
      let dialogOk = false
      // A page load the test did not ask for: a form submitted natively, or a crash reload.
      let navigating = 0
      for (const method of ['goto', 'reload', 'goBack', 'goForward'] as const) {
        const original = page[method].bind(page) as (...args: unknown[]) => Promise<unknown>
        ;(page as unknown as Record<string, unknown>)[method] = async (...args: unknown[]) => {
          navigating++
          try {
            return await original(...args)
          } finally {
            navigating--
          }
        }
      }
      page.on('request', r => {
        if (r.isNavigationRequest() && r.frame() === page.mainFrame() && !navigating)
          problems.push(`The page reloaded by itself: ${r.method()} ${new URL(r.url()).pathname}${new URL(r.url()).search}`)
      })
      page.on('pageerror', e => problems.push(`Uncaught in page: ${e.message}`))
      page.on('console', m => {
        if (m.type() !== 'error') return
        const text = m.text()
        // Failed responses are judged by the response check below.
        if (/^Failed to load resource: the server responded with a status of/.test(text)) return
        if (CONSOLE_ALLOWLIST.some(re => re.test(text))) return
        problems.push(`console.error: ${text}`)
      })
      page.on('response', r => {
        const url = new URL(r.url())
        if (r.status() < 400 || !NODE_ROUTES.test(url.pathname)) return
        if (allowed.some(a => a.status === r.status() && a.path.test(url.pathname))) return
        problems.push(`Node answered ${r.status()} to ${r.request().method()} ${url.pathname}`)
      })
      await use({
        allowStatus: (status, path) => void allowed.push({ status, path }),
        allowOpenDialog: () => void (dialogOk = true),
        problems: () => [...problems]
      })
      if (info.status !== info.expectedStatus) return
      if (!dialogOk && !page.isClosed()) {
        const open = await page.locator('dialog[open]').evaluateAll(ds => ds.map(d => d.getAttribute('aria-label') ?? 'dialog')).catch(() => [])
        for (const label of open) problems.push(`A "${label}" dialog was still open when the test ended`)
      }
      expect(problems, 'page errors, unexpected node errors, or stuck dialogs').toEqual([])
    },
    { auto: true }
  ]
})

export { expect }
