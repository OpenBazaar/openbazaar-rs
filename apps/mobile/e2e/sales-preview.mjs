// Optional, loopback-only UI fixture. No synthetic sale is written to a node.
import { createHash } from 'node:crypto'

const digest = value => createHash('sha256').update(value).digest('hex')
const products = [
  ['Stoneware mug · Slate', 'physical', 24000], ['Linen apron · Natural', 'physical', 18000],
  ['Cotton notebook set', 'physical', 6400], ['Glaze recipes (PDF)', 'digital', 5000],
  ['Brass pen holder', 'physical', 12500], ['Wholesale coffee · 12 bags', 'physical', 114000],
  ['Ceramic pourer', 'physical', 13000], ['Letterpress cards · 50 pack', 'physical', 48000]
]

export function simulatedSales(count, now = Math.floor(Date.now() / 1000)) {
  if (!Number.isInteger(count) || count < 1 || count > 5000) throw new Error('Choose 1–5,000 simulated orders.')
  return Array.from({ length: count }, (_, i) => {
    const [title, kind, priceSats] = products[i % products.length]
    const status = i % 10 < 3 ? 'Paid' : i % 10 < 5 ? 'Awaiting payment' : i % 10 === 5 ? 'Payment seen' : i % 10 === 6 ? 'Shipped' : 'Completed'
    const lines = [{ title, kind, priceSats, quantity: 1 + i % 5, listingHash: digest(title) }]
    if (kind === 'physical' && i % 7 === 0) lines.push({ title: 'Gift packaging', kind: 'physical', priceSats: 1200, quantity: 1, listingHash: digest('Gift packaging') })
    const itemsSats = lines.reduce((sum, line) => sum + line.priceSats * line.quantity, 0)
    const shippingSats = kind === 'physical' ? 3000 + i % 4 * 500 : 0
    const totalSats = itemsSats + shippingSats
    const confirmed = ['Paid', 'Shipped', 'Completed'].includes(status)
    const delivered = ['Shipped', 'Completed'].includes(status)
    const created = now - i * 900 - i % 7 * 37
    return {
      id: digest(`simulated-sale-${i}`).slice(0, 32), created, status, network: 'testnet4', lines,
      itemsSats, shippingSats, totalSats,
      address: 'Simulated address — not payable',
      shipTo: kind === 'physical' ? `Test buyer ${String(i + 1).padStart(4, '0')}\n${100 + i % 900} Example Street\nSample City, 00000` : '',
      deliveryNote: i % 6 === 0 ? 'Simulation: please include the packing slip.' : '',
      receivedSats: status === 'Awaiting payment' ? (i % 2 ? Math.floor(totalSats / 2) : 0) : totalSats,
      confirmations: confirmed ? 1 + i % 6 : 0,
      txids: status === 'Awaiting payment' ? [] : [digest(`simulated-transaction-${i}`)],
      sentTxid: null,
      fulfillment: delivered ? { carrier: kind === 'physical' ? 'Test carrier' : '', tracking: kind === 'physical' ? `SIM-${100000 + i}` : '', note: 'Simulated delivery record.', content: kind === 'digital' ? 'Example download instructions — no real goods.' : '', at: created + 3600 } : null
    }
  })
}

export function createSalesPreview(initialCount) {
  let sales = simulatedSales(initialCount)
  const json = (res, status, value) => { res.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); res.end(JSON.stringify(value)) }
  async function body(req) {
    let text = ''
    for await (const chunk of req) {
      text += chunk
      if (text.length > 8192) throw new Error('Request too large.')
    }
    return JSON.parse(text || '{}')
  }
  return async (req, res, path) => {
    if (req.method === 'GET' && path === '/__preview') {
      res.writeHead(200, { 'Content-Type': 'text/html', 'Cache-Control': 'no-store' })
      res.end(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>OpenBazaar · Simulated sales</title><style>*{box-sizing:border-box}body{margin:0;background:#0b1730;color:#f2f6fc;font:14px/1.4 Arial,sans-serif;height:100dvh;display:flex;flex-direction:column}header{display:flex;gap:12px 24px;align-items:center;justify-content:space-between;padding:10px 18px;flex-wrap:wrap;border-bottom:1px solid #2e4564}strong{font-size:15px}small{display:block;color:#b4c7dc}label{display:flex;gap:10px;align-items:center}select,button{font:inherit;color:#f2f6fc;background:#182b46;border:1px solid #41628a;border-radius:6px;min-height:36px;padding:6px 10px}iframe{flex:1;min-height:0;border:0;width:100%;background:#f7faff}@media(max-width:640px){header{padding:10px 12px;gap:8px}label{font-size:13px}select,button{min-height:40px}}</style><header><div><strong>Simulated sales console</strong><small>No funds or real shipments. Changes reset with this preview.</small></div><label>Test orders<select id="count"><option value="100">100</option><option value="1000">1,000</option><option value="5000">5,000</option></select><button id="reset" type="button">Load / reset</button></label></header><iframe title="OpenBazaar sales preview" src="/" allow="clipboard-read; clipboard-write"></iframe><script>document.querySelector('#count').value='${sales.length}';document.querySelector('#reset').onclick=async()=>{const button=document.querySelector('#reset');button.disabled=true;try{const response=await fetch('/__preview/reset',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({count:Number(document.querySelector('#count').value)})});if(!response.ok)throw new Error('Preview reset failed');document.querySelector('iframe').contentWindow.location.reload()}catch{button.textContent='Try again'}finally{button.disabled=false}};</script></html>`)
      return true
    }
    if (req.method === 'GET' && path === '/sales') { json(res, 200, { sales }); return true }
    if (req.method === 'GET' && path === '/store/profile') {
      json(res, 200, { role: 'seller', storeKey: digest('simulated-store'), name: 'Simulation store', setUp: true, network: 'testnet4', published: false, listingCount: 0 }); return true
    }
    if (req.method === 'GET' && path === '/wallet') {
      json(res, 200, { network: 'testnet4', balance: { totalSats: 0, confirmedSats: 0, pendingSats: 0, spendableSats: 0 }, receiveAddress: '', lastSync: null, syncError: null, activity: [] }); return true
    }
    if (req.method === 'GET' && /^\/sales\/[0-9a-f]+$/.test(path)) {
      const sale = sales.find(sale => sale.id === path.split('/').at(-1)); json(res, sale ? 200 : 404, sale ?? { error: 'No simulated order with that ID.' }); return true
    }
    if (req.method === 'POST' && path === '/__preview/reset') {
      try { sales = simulatedSales(Number((await body(req)).count)); json(res, 200, { count: sales.length }) }
      catch { json(res, 400, { error: 'Choose 1–5,000 simulated orders.' }) }
      return true
    }
    if (req.method === 'POST' && /^\/sales\/[0-9a-f]+\/fulfill$/.test(path)) {
      const sale = sales.find(sale => sale.id === path.split('/').at(-2))
      if (!sale || sale.status !== 'Paid') { json(res, 409, { error: 'This simulated order is not ready for fulfillment.' }); return true }
      try {
        const details = await body(req)
        sale.fulfillment = { ...Object.fromEntries(['carrier', 'tracking', 'note', 'content'].map(key => [key, typeof details[key] === 'string' ? details[key].slice(0, 4000) : ''])), at: Math.floor(Date.now() / 1000) }
        sale.status = 'Shipped'
        json(res, 200, sale)
      } catch { json(res, 400, { error: 'The simulated delivery could not be read.' }) }
      return true
    }
    if (req.method !== 'GET' && req.method !== 'HEAD') { json(res, 405, { error: 'Simulation only: real payments and node changes are disabled.' }); return true }
    return false
  }
}
