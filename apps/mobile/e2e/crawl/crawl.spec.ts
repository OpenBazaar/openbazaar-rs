import { writeFile } from 'node:fs/promises'
import { test, expect, PASSPHRASE, type TestNode } from '../real/harness.ts'
import { crawl, type Finding } from './engine.ts'
import { KNOWN } from './known.ts'

// Explores the shop on real nodes. OB_CRAWL_SEED replays a run; OB_CRAWL_MINUTES
// sets the time budget (shared by the buyer, seller and moderator pages).
const seed = Number(process.env.OB_CRAWL_SEED || Date.now() % 2147483647)
const minutes = Number(process.env.OB_CRAWL_MINUTES || 2)

test('crawl the buyer, seller and moderator pages for dead controls, stuck dialogs and errors', async ({ page, market, chain, guard }, info) => {
  const { moderator, seller, buyer } = market
  // Validation refusals and locked-wallet answers are expected while fuzzing.
  for (const status of [400, 409, 422, 423]) guard.allowStatus(status, /./)
  guard.allowOpenDialog()
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write'])

  chain.fund(await buyer.receiveAddress(), 500000)
  chain.fund(await seller.receiveAddress(), 100000)
  for (const [n, title] of ['Ash glaze mug', 'Linen tea towel', 'Field notes zine'].entries())
    await seller.addListing({ title, kind: n === 2 ? 'digital' : 'physical', price: 12000 + n * 1000, images: [await seller.uploadImage(), await seller.uploadImage()] })
  await buyer.follow(seller)
  await expect.poll(async () => (await buyer.balance()).spendableSats, { timeout: 20000 }).toBe(500000)

  const log: string[] = [`seed ${seed}`]
  // Node errors go in the log right after the action that caused them.
  page.on('response', r => {
    if (r.status() >= 400 && !r.url().match(/\.(js|css|png|jpg|woff2)$/)) log.push(`  -> ${r.status()} ${r.request().method()} ${new URL(r.url()).pathname}`)
  })
  console.log(`Crawl seed ${seed}. Replay with OB_CRAWL_SEED=${seed}.`)
  const findings: (Finding & { node: string })[] = []
  const budget = (minutes * 60000) / 3
  for (const node of [buyer, seller, moderator] as TestNode[]) {
    await page.goto(node.ui)
    const found = await crawl(page, {
      seed,
      maxSteps: 100000,
      maxMs: budget,
      passphrase: PASSPHRASE,
      home: node.ui,
      chaos: async () => void (await node.lock().catch(() => {})),
      chaosRate: 0.03,
      log: line => log.push(`${node.name} ${line}`),
      screenshot: async name => {
        const path = info.outputPath(`${node.name}-${name}.png`)
        await page.screenshot({ path }).catch(() => {})
        return path
      }
    })
    const unknown = found.filter(f => !KNOWN.some(k => k.kind === f.kind && k.screen.test(f.screen) && k.control.test(f.control)))
    findings.push(...unknown.map(f => ({ ...f, node: node.name })))
  }

  await writeFile(info.outputPath('crawl-log.txt'), log.join('\n'))
  await writeFile(info.outputPath('findings.json'), JSON.stringify({ seed, findings, problems: guard.problems() }, null, 2))
  await info.attach('crawl-log.txt', { path: info.outputPath('crawl-log.txt'), contentType: 'text/plain' })
  await info.attach('findings.json', { path: info.outputPath('findings.json'), contentType: 'application/json' })
  for (const f of findings) if (f.screenshot) await info.attach(`${f.node}: ${f.kind} ${f.control}`, { path: f.screenshot, contentType: 'image/png' })
  expect(findings.map(f => `${f.node}: ${f.kind} on ${f.screen} › ${f.control} (step ${f.step}): ${f.detail}`), `crawl seed ${seed}`).toEqual([])
})
