// Disposable real node + same-origin static/proxy server. No existing app/node is touched.
import { createServer, request } from 'node:http'
import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, dirname, extname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
const app = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const uiPort = Number(process.env.OB_E2E_UI_PORT || 48173),
  nodePort = Number(process.env.OB_E2E_NODE_PORT || 48187)
if (
  [uiPort, nodePort].some(
    p =>
      !Number.isInteger(p) || p < 1024 || p > 65535 || p === 8787 || p === 4173
  ) ||
  uiPort === nodePort
)
  throw new Error('Use two spare ports other than 8787/4173.')
const data = await mkdtemp(join(tmpdir(), 'ob-ui-e2e-'))
const binary =
  process.env.OB_NODE_BIN || resolve(app, '../../target/debug/ob-node')
// Fail chain calls quickly and locally. E2E covers the client, not chain integration.
const node = spawn(
  binary,
  [
    'serve',
    String(nodePort),
    '--network',
    'testnet4',
    '--data-dir',
    data,
    '--esplora',
    process.env.OB_E2E_CHAIN_URL || `http://127.0.0.1:${uiPort}/chain`,
    '--poll-secs',
    '60',
    // A disposable node must not write to the user's OS keychain.
    '--vault',
    'file'
  ],
  { stdio: ['ignore', 'ignore', 'pipe'] }
)
let nodeError = ''
node.stderr.on('data', b => {
  nodeError = (
    nodeError +
    b.toString().replace(/pairing token:[^\n]*/gi, 'pairing token: [redacted]')
  ).slice(-3000)
})
node.on('error', e => {
  console.error(e.message)
  process.exitCode = 1
})
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
const server = createServer(async (req, res) => {
  const path = new URL(req.url, 'http://localhost').pathname
  if (path.startsWith('/chain')) {
    res.writeHead(503)
    res.end('Chain integration is outside this UI test')
    return
  }
  if (
    /^\/(catalog|rates|network|wallet|pair|orders|checkout|addresses|labelers|vendor-labels|listings|lock|unlock|setup|spend-tickets)(\/|$)/.test(
      path
    )
  ) {
    const headers = { ...req.headers, host: `127.0.0.1:${nodePort}` }
    delete headers.origin
    delete headers.referer
    const proxy = request(
      {
        hostname: '127.0.0.1',
        port: nodePort,
        path: req.url,
        method: req.method,
        headers
      },
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
    const relative =
      path === '/' ? 'index.html' : decodeURIComponent(path.slice(1))
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
            `<head><script>globalThis.__OB_NODE_ORIGIN__=location.origin;${await pairTokenScript()}</script>`
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
// Hand the page the node's token the way the desktop app does. A browser shop
// would have to type a code from the node's terminal.
async function pairTokenScript() {
  try {
    const token = (
      await readFile(join(data, 'testnet4', 'pairing.token'), 'utf8')
    ).trim()
    return /^[0-9a-f]{64}$/i.test(token)
      ? `globalThis.__OB_PAIR_TOKEN__=${JSON.stringify(token)};`
      : ''
  } catch {
    return ''
  }
}
server.listen(uiPort, '127.0.0.1')
let stopping = false
async function stop() {
  if (stopping) return
  stopping = true
  node.kill('SIGTERM')
  server.closeAllConnections()
  server.close()
  await rm(data, { recursive: true, force: true })
  process.exit()
}
process.on('SIGTERM', stop)
process.on('SIGINT', stop)
node.on('exit', code => {
  if (!stopping && code) {
    console.error(nodeError)
    void stop()
  }
})
