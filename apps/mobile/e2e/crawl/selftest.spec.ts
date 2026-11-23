import { test, expect } from '@playwright/test'
import { createServer, type Server } from 'node:http'
import { crawl } from './engine.ts'

// The crawler must find a planted dead button and a planted dialog that will not close.
const PLANTED = `<!doctype html><html><body>
<h1>Planted page</h1>
<p id="count">0</p>
<button onclick="document.getElementById('count').textContent++">Count</button>
<button>Does nothing</button>
<button onclick="document.getElementById('trap').showModal()">Open trap</button>
<dialog id="trap" aria-label="Trap" oncancel="event.preventDefault()"><h2>Trap</h2><p>No way out.</p><button>Cancel</button></dialog>
</body></html>`

let server: Server
let home = ''
test.beforeAll(async () => {
  server = createServer((_, res) => res.writeHead(200, { 'Content-Type': 'text/html' }).end(PLANTED))
  await new Promise<void>(r => server.listen(0, '127.0.0.1', r))
  home = `http://127.0.0.1:${(server.address() as { port: number }).port}/`
})
test.afterAll(() => new Promise<void>(r => server.close(() => r())))

test('the crawler reports a dead button and a dialog that cannot be closed, and nothing else', async ({ page }) => {
  await page.goto(home)
  const findings = await crawl(page, { seed: 7, maxSteps: 25, maxMs: 60000, passphrase: 'x', home })
  const kinds = findings.map(f => `${f.kind}: ${f.control}`)
  expect(kinds).toContain('dead control: Does nothing')
  expect(findings.some(f => f.kind === 'stuck dialog' && f.screen.includes('Trap'))).toBe(true)
  expect(kinds).not.toContain('dead control: Count')
  expect(kinds).not.toContain('dead control: Open trap')
})

test('a number field and a section link are not reported as broken', async ({ page }) => {
  const pageServer = createServer((_, res) => res.writeHead(200, { 'Content-Type': 'text/html' }).end(`<!doctype html><html><body>
<h1>Filters</h1>
<label>Max. price in sats<input type="number" min="0" placeholder="No maximum"></label>
<a href="#photos">Photos</a>
<section id="photos"><h2>Photos</h2></section>
</body></html>`))
  await new Promise<void>(r => pageServer.listen(0, '127.0.0.1', r))
  const url = `http://127.0.0.1:${(pageServer.address() as { port: number }).port}/`
  try {
    await page.goto(url)
    const findings = await crawl(page, { seed: 1, maxSteps: 12, maxMs: 30000, passphrase: 'x', home: url })
    expect(findings).toEqual([])
  } finally {
    await new Promise<void>(r => pageServer.close(() => r()))
  }
})
