// A disposable real ob-node and the same-origin static/proxy server in front of
// it. Shared by server.mjs (the mocked project) and the real-node fixtures.
import { createServer, request } from 'node:http'
import { spawn } from 'node:child_process'
import { createWriteStream } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, dirname, extname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export const app = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const binary = process.env.OB_NODE_BIN || resolve(app, '../../target/debug/ob-node')

export const NODE_ROUTES =
  /^\/(catalog|rates|network|wallet|pair|orders|checkout|addresses|labelers|content-labels|vendor-labels|listings|directory|follow|lock|unlock|setup|spend-tickets|stores|media|store|sales|follows|home|remote|ln|offers|coupons|xmr|zec|moderator|moderators|reviews|web|version|browser-session|escrow)(\/|$)/

const redact = text => text.replace(/pairing token:[^\n]*/gi, 'pairing token: [redacted]')

/**
 * Starts `ob-node serve` on testnet4 in a new temporary data directory, with
 * the file vault so the OS keychain is never touched. `chainUrl` defaults to
 * the UI server's `/chain`. Resolves once the node answers `/network`.
 */
export async function startNode({ port, uiPort, role = null, chainUrl, pollSecs, logFile, extraArgs = [], onExit }) {
  const data = await mkdtemp(join(tmpdir(), 'ob-ui-e2e-'))
  if (role) {
    await mkdir(join(data, 'testnet4'), { recursive: true })
    await writeFile(join(data, 'testnet4', 'store-profile.json'), JSON.stringify({ role }))
  }
  const child = spawn(
    binary,
    [
      'serve', String(port),
      '--network', 'testnet4',
      '--data-dir', data,
      '--esplora', chainUrl || `http://127.0.0.1:${uiPort}/chain`,
      '--poll-secs', String(pollSecs ?? 60),
      '--vault', 'file',
      ...extraArgs
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] }
  )
  let tail = ''
  const log = logFile ? createWriteStream(logFile) : null
  const sink = b => {
    const text = redact(b.toString())
    tail = (tail + text).slice(-3000)
    log?.write(text)
  }
  child.stdout.on('data', sink)
  child.stderr.on('data', sink)
  child.on('exit', code => onExit?.(code, tail))
  const node = {
    port,
    data,
    child,
    tail: () => tail,
    async stop() {
      if (child.exitCode === null) {
        const exited = new Promise(r => child.once('exit', r))
        child.kill('SIGTERM')
        await Promise.race([exited, new Promise(r => setTimeout(r, 5000))])
        if (child.exitCode === null) child.kill('SIGKILL')
      }
      log?.end()
      await rm(data, { recursive: true, force: true })
    }
  }
  return node
}

/** Polls until the node answers, or throws with its last log lines. */
export async function waitForNode(node, timeoutMs = 30000) {
  const until = Date.now() + timeoutMs
  while (Date.now() < until) {
    if (node.child.exitCode !== null) break
    try {
      const r = await fetch(`http://127.0.0.1:${node.port}/network`)
      if (r.status < 500) return
    } catch {}
    await new Promise(r => setTimeout(r, 100))
  }
  throw new Error(`ob-node on ${node.port} did not start:\n${node.tail()}`)
}

const mime = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.woff2': 'font/woff2',
  '.json': 'application/json'
}

/**
 * Serves `dist` with the desktop's origin and pairing-token injection and
 * proxies node routes. `/chain` goes to `chain(req, res, path)` when given,
 * else fails fast: chain integration is not what these pages test.
 */
export function startUi({ uiPort, nodePort, data, preview = null, chain = null }) {
  const server = createServer(async (req, res) => {
    const path = new URL(req.url, 'http://localhost').pathname
    if (preview && await preview(req, res, path)) return
    if (path.startsWith('/chain')) {
      if (chain) return chain(req, res, path.slice('/chain'.length) || '/')
      res.writeHead(503)
      res.end('Chain integration is outside this UI test')
      return
    }
    if (NODE_ROUTES.test(path)) {
      const headers = { ...req.headers, host: `127.0.0.1:${nodePort}` }
      delete headers.origin
      delete headers.referer
      const proxy = request(
        { hostname: '127.0.0.1', port: nodePort, path: req.url, method: req.method, headers },
        upstream => {
          res.writeHead(upstream.statusCode, upstream.headers)
          upstream.pipe(res)
        }
      )
      proxy.on('error', () => {
        res.writeHead(502)
        res.end('Test node not ready')
      })
      req.pipe(proxy)
      return
    }
    try {
      const relative = path === '/' ? 'index.html' : decodeURIComponent(path.slice(1))
      const file = resolve(app, 'dist', relative)
      if (!file.startsWith(resolve(app, 'dist') + '/')) {
        res.writeHead(403)
        res.end()
        return
      }
      let body = await readFile(file)
      if (relative === 'index.html')
        body = Buffer.from(
          body
            .toString()
            .replace(
              '<head>',
              `<head><script>globalThis.__OB_NODE_ORIGIN__=location.origin;${await pairTokenScript(data)}</script>`
            )
        )
      res.writeHead(200, {
        'Content-Type': mime[extname(file)] || 'application/octet-stream',
        'Cache-Control': 'no-store'
      })
      res.end(body)
    } catch {
      res.writeHead(404)
      res.end()
    }
  })
  server.listen(uiPort, '127.0.0.1')
  return server
}

export async function pairToken(data) {
  try {
    const token = (await readFile(join(data, 'testnet4', 'pairing.token'), 'utf8')).trim()
    return /^[0-9a-f]{64}$/i.test(token) ? token : null
  } catch {
    return null
  }
}

// Hand the page the node's token the way the desktop app does. A browser shop
// would have to type a code from the node's terminal.
async function pairTokenScript(data) {
  const token = await pairToken(data)
  return token ? `globalThis.__OB_PAIR_TOKEN__=${JSON.stringify(token)};` : ''
}
