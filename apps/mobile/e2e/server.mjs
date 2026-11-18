// Disposable real nodes + same-origin static/proxy servers. No existing app/node is touched.
// The shop node (UI on OB_E2E_UI_PORT) is a buyer; a second, fresh node (UI on
// OB_E2E_SELLER_UI_PORT) goes through onboarding as a seller.
import { createSalesPreview } from './sales-preview.mjs'
import { startNode as spawnNode, startUi as serveUi } from './nodes.mjs'
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
const nodes = []
let stopping = false

// Fail chain calls quickly and locally. E2E covers the client, not chain integration.
async function startNode(port, uiPort, role) {
  const node = await spawnNode({
    port,
    uiPort,
    role,
    chainUrl: process.env.OB_E2E_CHAIN_URL,
    onExit(code, tail) {
      if (!stopping && code) {
        console.error(tail)
        void stop()
      }
    }
  })
  nodes.push(node)
  return node.data
}

function startUi(uiPort, nodePort, data) {
  const preview = process.env.OB_SALES_PREVIEW_COUNT ? createSalesPreview(Number(process.env.OB_SALES_PREVIEW_COUNT)) : null
  return serveUi({ uiPort, nodePort, data, preview })
}

const servers = [
  startUi(ports.ui, ports.node, await startNode(ports.node, ports.ui, 'buyer')),
  startUi(ports.sellerUi, ports.sellerNode, await startNode(ports.sellerNode, ports.sellerUi, null))
]
async function stop() {
  if (stopping) return
  stopping = true
  for (const server of servers) {
    server.closeAllConnections()
    server.close()
  }
  await Promise.all(nodes.map(node => node.stop()))
  process.exit()
}
process.on('SIGTERM', stop)
process.on('SIGINT', stop)
