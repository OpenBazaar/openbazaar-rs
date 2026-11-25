// An exploratory crawler: it clicks every control it can reach in a seeded
// random order, fills inputs, and reports controls that do nothing, dialogs
// that cannot be closed, and error screens. Page errors, console errors and
// node errors come from the real-node guard around it.
import type { Locator, Page } from '@playwright/test'

export type Finding = {
  kind: 'dead control' | 'stuck dialog' | 'error screen' | 'click failed'
  screen: string
  control: string
  detail: string
  step: number
  screenshot?: string
}

export type CrawlOptions = {
  seed: number
  /** Stop after this many actions or this many milliseconds, whichever is first. */
  maxSteps: number
  maxMs: number
  /** Typed into inputs whose label mentions a passphrase. */
  passphrase: string
  /** Called with probability `chaosRate` before an action, e.g. to lock the wallet. */
  chaos?: () => Promise<void>
  chaosRate?: number
  /** Where to go when the crawl leaves the app or a screen is a dead end. */
  home: string
  log?: (line: string) => void
  screenshot?: (name: string) => Promise<string | undefined>
}

// Controls the crawler never presses: leaving the app, or undoing the setup it explores.
const SKIP = /^(skip to content)$|delete wallet|reset|forget|wipe|sign out|log out|remove all|rotate|dark mode|collapse sidebar|resize sidebar/i
const CLOSE = /^(close|cancel|done|not now|dismiss|×|back)$/i
const FUZZ = ['', 'a', 'Test value 123', '0', '-1', '99999999999', 'ñandú 🚀', '<b>x</b>', ' '.repeat(3), 'x'.repeat(300)]
// A number input rejects anything that is not a number, and Playwright reports that as a failed action.
const NUMBER_FUZZ = ['0', '1', '12', '999999']

/** Mulberry32: small, seedable, good enough to pick controls. */
export function rng(seed: number) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const CONTROLS = 'button, a[href], summary, select, [role=tab], [role=switch], [role=menuitem], [role=checkbox], [role=radio], input[type=checkbox], input[type=radio], input:not([type]), input[type=text], input[type=number], input[type=search], input[type=password], input[type=email], input[type=url], textarea'

type Control = { index: number; name: string; tag: string; kind: string; current: boolean }

/** What the user can see and what the controls say, to tell whether a click did anything. */
async function signature(page: Page): Promise<string> {
  return page.evaluate(() => {
    const states = [...document.querySelectorAll('[aria-pressed],[aria-expanded],[aria-checked],[aria-selected],details,input,select,textarea,dialog')]
      .map(e => [e.getAttribute('aria-pressed'), e.getAttribute('aria-expanded'), e.getAttribute('aria-checked'), e.getAttribute('aria-selected'), (e as HTMLDetailsElement).open, (e as HTMLInputElement).value, (e as HTMLInputElement).checked, (e as HTMLDialogElement).open].join(','))
      .join('|')
    const text = `${document.body.innerText}#${states}`
    let hash = 5381
    for (let i = 0; i < text.length; i++) hash = (Math.imul(hash, 33) ^ text.charCodeAt(i)) >>> 0
    return `${location.href}#${text.length}#${hash}`
  })
}

/** Waits until the page stops changing for a moment, at most `maxMs`. */
async function settle(page: Page, maxMs = 1500) {
  const until = Date.now() + maxMs
  let last = await signature(page).catch(() => '')
  while (Date.now() < until) {
    await page.waitForTimeout(150)
    const next = await signature(page).catch(() => '')
    if (next === last) return
    last = next
  }
}

async function screenName(page: Page): Promise<string> {
  return page.evaluate(() => {
    const dialog = [...document.querySelectorAll('dialog[open]')].pop()
    const heading = (dialog ?? document).querySelector('h1, h2')
    const label = dialog?.getAttribute('aria-label')
    return [label && `dialog "${label}"`, heading?.textContent?.trim().slice(0, 60)].filter(Boolean).join(' › ') || location.pathname
  }).catch(() => 'unknown')
}

/** The open dialog when there is one: a modal blocks everything else. */
function scope(page: Page): Locator {
  return page.locator('dialog[open]').last().or(page.locator('body').filter({ hasNot: page.locator('dialog[open]') }))
}

async function controls(page: Page): Promise<Control[]> {
  const root = scope(page)
  return root.locator(CONTROLS).evaluateAll((els, skip) => {
    const re = new RegExp(skip, 'i')
    return els.flatMap((e, index) => {
      const el = e as HTMLElement
      const rect = el.getBoundingClientRect()
      const style = getComputedStyle(el)
      if (!rect.width || !rect.height || style.visibility === 'hidden' || style.display === 'none') return []
      if ((el as HTMLButtonElement).disabled || el.getAttribute('aria-disabled') === 'true' || el.closest('[data-crawl=skip],[inert]')) return []
      const a = el as HTMLAnchorElement
      if (el.tagName === 'A' && (a.target === '_blank' || (a.href && new URL(a.href).origin !== location.origin))) return []
      const name = (el.getAttribute('aria-label') || el.innerText || (el as HTMLInputElement).placeholder || el.getAttribute('name') || el.tagName).trim().replace(/\s+/g, ' ').slice(0, 80)
      if (re.test(name)) return []
      const kind = el instanceof HTMLInputElement ? el.type : el.getAttribute('role') || el.tagName.toLowerCase()
      // Pressing what is already current or selected may rightly do nothing.
      const current = [el.getAttribute('aria-current'), el.getAttribute('aria-selected'), el.getAttribute('aria-pressed'), el.getAttribute('aria-checked')].some(v => v && v !== 'false')
      return [{ index, name, tag: el.tagName.toLowerCase(), kind, current }]
    })
  }, SKIP.source)
}

async function labelOf(handle: Locator): Promise<string> {
  return handle.evaluate(el => {
    const id = el.id
    const label = (id && document.querySelector(`label[for="${id}"]`)) || el.closest('label')
    return `${el.getAttribute('aria-label') ?? ''} ${label?.textContent ?? ''} ${(el as HTMLInputElement).placeholder ?? ''}`
  })
}

/** Tries to close the open dialog the ways a person would. */
async function closeDialog(page: Page): Promise<boolean> {
  const open = () => page.locator('dialog[open]').count()
  if (!(await open())) return true
  await page.keyboard.press('Escape').catch(() => {})
  await page.waitForTimeout(200)
  if (!(await open())) return true
  const dialog = page.locator('dialog[open]').last()
  for (const button of await dialog.getByRole('button').all()) {
    const name = ((await button.getAttribute('aria-label')) || (await button.innerText().catch(() => ''))).trim()
    if (!CLOSE.test(name)) continue
    await button.click({ timeout: 2000 }).catch(() => {})
    await page.waitForTimeout(200)
    if (!(await open())) return true
  }
  return false
}

/** A second press once a toast or other passing feedback from the first has gone. */
async function stillDead(page: Page, handle: Locator): Promise<boolean> {
  await page.waitForTimeout(3500)
  const before = await signature(page)
  let requests = 0
  const count = () => void requests++
  page.on('request', count)
  try {
    await handle.click({ timeout: 3000 })
  } catch {
    return false
  } finally {
    await settle(page)
    page.off('request', count)
  }
  return requests === 0 && (await signature(page)) === before
}

export async function crawl(page: Page, options: CrawlOptions): Promise<Finding[]> {
  const random = rng(options.seed)
  const log = options.log ?? (() => {})
  const findings: Finding[] = []
  const seen = new Set<string>()
  const pressed = new Map<string, number>()
  const started = Date.now()
  let dialogSteps = 0
  const report = async (finding: Omit<Finding, 'screenshot'>) => {
    const key = `${finding.kind}|${finding.screen}|${finding.control}`
    if (seen.has(key)) return
    seen.add(key)
    const screenshot = await options.screenshot?.(`finding-${findings.length + 1}`)
    findings.push({ ...finding, screenshot })
    log(`FINDING ${finding.kind}: ${finding.screen} › ${finding.control}: ${finding.detail}`)
  }

  for (let step = 1; step <= options.maxSteps && Date.now() - started < options.maxMs; step++) {
    if (!page.url().startsWith(new URL(options.home).origin)) await page.goto(options.home)
    if (options.chaos && random() < (options.chaosRate ?? 0)) {
      log(`${step}: chaos`)
      await options.chaos()
    }
    // Explore a dialog for a few steps, then it must close the normal way.
    dialogSteps = (await page.locator('dialog[open]').count()) ? dialogSteps + 1 : 0
    if (dialogSteps > 4) {
      dialogSteps = 0
      if (!(await closeDialog(page))) {
        await report({ kind: 'stuck dialog', screen: await screenName(page), control: '', detail: 'Escape and its Close/Cancel/Done buttons did not close it', step })
        await page.goto(options.home)
      }
      continue
    }
    const screen = await screenName(page)
    if (await page.getByText(/something went wrong|unexpected error|cannot read properties/i).count())
      await report({ kind: 'error screen', screen, control: '', detail: 'The page shows an error screen', step })

    const list = await controls(page)
    if (!list.length) {
      if (!(await closeDialog(page))) {
        await report({ kind: 'stuck dialog', screen, control: '', detail: 'A dialog with nothing to press could not be closed', step })
        await page.goto(options.home)
      }
      continue
    }
    // Prefer controls pressed least, so the crawl spreads across the app.
    const weights = list.map(c => 1 / (1 + (pressed.get(`${screen}|${c.name}`) ?? 0)))
    let pick = random() * weights.reduce((a, b) => a + b, 0)
    const control = list[Math.max(0, weights.findIndex(w => (pick -= w) <= 0))] ?? list[0]
    const key = `${screen}|${control.name}`
    pressed.set(key, (pressed.get(key) ?? 0) + 1)
    const handle = scope(page).locator(CONTROLS).nth(control.index)
    log(`${step}: ${screen} › ${control.kind} "${control.name}"`)

    const before = await signature(page)
    const requests: string[] = []
    const onRequest = (r: { url: () => string }) => void requests.push(r.url())
    page.on('request', onRequest)
    const dialogsBefore = await page.locator('dialog[open]').count()
    let acted = true
    try {
      if (['text', 'number', 'search', 'password', 'email', 'url', 'textarea'].includes(control.kind) || control.tag === 'textarea') {
        const label = await labelOf(handle)
        const pool = control.kind === 'number' ? NUMBER_FUZZ : FUZZ
        const value = /passphrase/i.test(label) && control.kind !== 'number' ? options.passphrase : pool[Math.floor(random() * pool.length)]
        // Playwright's fill() clears with an empty string first; some engines then refuse
        // to type into input[type=number] ("Cannot type text into input[type=number]").
        if (control.kind === 'number') {
          await handle.evaluate((el, next) => {
            const input = el as HTMLInputElement
            input.focus()
            input.value = next
            input.dispatchEvent(new Event('input', { bubbles: true }))
            input.dispatchEvent(new Event('change', { bubbles: true }))
          }, value)
        } else {
          await handle.fill(value, { timeout: 3000 })
        }
        await handle.press(random() < 0.3 ? 'Enter' : 'Tab', { timeout: 3000 })
      } else if (control.tag === 'select') {
        const values = await handle.locator('option').evaluateAll(os => os.map(o => (o as HTMLOptionElement).value))
        if (values.length) await handle.selectOption(values[Math.floor(random() * values.length)], { timeout: 3000 })
      } else {
        await handle.click({ timeout: 3000 })
      }
    } catch (e) {
      acted = false
      // Timeouts and covered or moving elements are the crawler's problem, not the page's.
      const message = (e as Error).message.split('\n')[0]
      if (!/Timeout|detached|not attached|outside of the viewport|intercepts pointer events|Target page|closed/.test(message))
        await report({ kind: 'click failed', screen, control: control.name, detail: message, step })
    }
    await settle(page)
    page.off('request', onRequest)
    const after = await signature(page).catch(() => before)
    const isPressable = ['button', 'summary', 'tab', 'switch', 'menuitem', 'a'].includes(control.kind) || control.tag === 'button'
    // A visible error already answers the press, e.g. pressing Review again on an invalid form.
    const answered = (await scope(page).getByRole('alert').filter({ visible: true }).count()) > 0
    if (acted && isPressable && !control.current && !answered && after === before && !requests.length && (await stillDead(page, handle))) {
      const href = control.tag === 'a' ? await handle.getAttribute('href') : null
      const section = href?.startsWith('#') ? href.slice(1) : ''
      // A section link that is already showing did its job. One that names no section did not.
      if (section && await page.evaluate(id => !!document.getElementById(id), section)) continue
      await report({
        kind: 'dead control',
        screen,
        control: control.name,
        detail: section ? 'This link points at a section that is not on the page' : 'Pressing it changed nothing on the page and sent nothing to the node, twice',
        step
      })
    }

    // A dialog opened by this step must close again the normal way.
    if ((await page.locator('dialog[open]').count()) > dialogsBefore && random() < 0.5) {
      if (!(await closeDialog(page))) {
        await report({ kind: 'stuck dialog', screen: await screenName(page), control: control.name, detail: 'Escape and its Close/Cancel/Done buttons did not close it', step })
        await page.goto(options.home)
      }
    }
  }
  return findings
}
