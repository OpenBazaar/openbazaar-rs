// Disposable real nodes + same-origin static/proxy servers. No existing app/node is touched.
// The shop node (UI on OB_E2E_UI_PORT) is a buyer; a second, fresh node (UI on
// OB_E2E_SELLER_UI_PORT) goes through onboarding as a seller.
import { createServer, request } from 'node:http'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, dirname, extname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
const app = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const ports = {
  ui: Number(process.env.OB_E2E_UI_PORT || 48173),
  node: Number(process.env.OB_E2E_NODE_PORT || 48187),
  sellerUi: Number(process.env.OB_E2E_SELLER_UI_PORT || 48174),
  sellerNode: Number(process.env.OB_E2E_SELLER_NODE_PORT || 48188)
}
const all = Object.values(ports)
if (
  all.some(
    p =>
      !Number.isInteger(p) || p < 1024 || p > 65535 || p === 8787 || p === 4173
  ) ||
  new Set(all).size !== all.length
)
  throw new Error('Use four spare ports other than 8787/4173.')
const binary =
  process.env.OB_NODE_BIN || resolve(app, '../../target/debug/ob-node')
const roots = []
const nodes = []
let stopping = false

// Fail chain calls quickly and locally. E2E covers the client, not chain integration.
async function startNode(port, uiPort, role) {
  const data = await mkdtemp(join(tmpdir(), 'ob-ui-e2e-'))
  roots.push(data)
  if (role) {
    await mkdir(join(data, 'testnet4'), { recursive: true })
    await writeFile(join(data, 'testnet4', 'store-profile.json'), JSON.stringify({ role }))
  }
  const node = spawn(
    binary,
    [
      'serve',
      String(port),
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
  node.on('exit', code => {
    if (!stopping && code) {
      console.error(nodeError)
      void stop()
    }
  })
  nodes.push(node)
  return data
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
const NODE_ROUTES =
  /^\/(catalog|rates|network|wallet|pair|orders|checkout|addresses|labelers|content-labels|vendor-labels|listings|follow|lock|unlock|setup|spend-tickets|stores|media|store|sales|follows|home|remote|ln|offers|coupons|xmr)(\/|$)/

function startUi(uiPort, nodePort, data) {
  const server = createServer(async (req, res) => {
    const path = new URL(req.url, 'http://localhost').pathname
    if (path.startsWith('/chain')) {
      res.writeHead(503)
      res.end('Chain integration is outside this UI test')
      return
    }
    if (NODE_ROUTES.test(path)) {
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
// Hand the page the node's token the way the desktop app does. A browser shop
// would have to type a code from the node's terminal.
async function pairTokenScript(data) {
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

const servers = [
  startUi(ports.ui, ports.node, await startNode(ports.node, ports.ui, 'buyer')),
  startUi(ports.sellerUi, ports.sellerNode, await startNode(ports.sellerNode, ports.sellerUi, null))
]
async function stop() {
  if (stopping) return
  stopping = true
  for (const node of nodes) node.kill('SIGTERM')
  for (const server of servers) {
    server.closeAllConnections()
    server.close()
  }
  await Promise.all(roots.map(data => rm(data, { recursive: true, force: true })))
  process.exit()
}
process.on('SIGTERM', stop)
process.on('SIGINT', stop)
