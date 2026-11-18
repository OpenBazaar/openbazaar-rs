// A simulated testnet4 chain behind the Esplora HTTP API, enough for ob-node's
// BDK wallet sync, payment watching, and broadcasts. Transactions are real
// serialized transactions with real txids; blocks are only ids and heights.
// Tests fund addresses and mine blocks directly; nothing leaves this process.
import { createHash, randomBytes } from 'node:crypto'
import { createServer } from 'node:http'

const TESTNET4_GENESIS = '00000000da84f2bafbbc53dee25a72ae507ff4914b867c565be350b0da8bf043'
const START_HEIGHT = 100
const BLOCK_SECS = 600

const sha256 = b => createHash('sha256').update(b).digest()
const dsha = b => sha256(sha256(b))
const reverseHex = b => Buffer.from(b).reverse().toString('hex')

// --- bech32 / bech32m (BIP 173, BIP 350) ---
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l'
function polymod(values) {
  const G = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3]
  let chk = 1
  for (const v of values) {
    const top = chk >>> 25
    chk = ((chk & 0x1ffffff) << 5) ^ v
    for (let i = 0; i < 5; i++) if ((top >>> i) & 1) chk ^= G[i]
  }
  return chk >>> 0
}
const hrpExpand = hrp => [...hrp].map(c => c.charCodeAt(0) >> 5).concat([0], [...hrp].map(c => c.charCodeAt(0) & 31))
function convertBits(data, from, to) {
  let acc = 0
  let bits = 0
  const out = []
  for (const value of data) {
    acc = (acc << from) | value
    bits += from
    while (bits >= to) {
      bits -= to
      out.push((acc >> bits) & ((1 << to) - 1))
    }
  }
  return out
}
/** The output script for a testnet segwit address (tb1q… or tb1p…). */
export function scriptForAddress(address) {
  const lower = address.toLowerCase()
  const split = lower.lastIndexOf('1')
  const hrp = lower.slice(0, split)
  if (hrp !== 'tb') throw new Error(`Not a testnet segwit address: ${address}`)
  const data = [...lower.slice(split + 1)].map(c => CHARSET.indexOf(c))
  if (data.some(d => d < 0)) throw new Error(`Bad bech32 address: ${address}`)
  const check = polymod(hrpExpand(hrp).concat(data))
  const version = data[0]
  if (check !== (version === 0 ? 1 : 0x2bc830a3)) throw new Error(`Bad checksum: ${address}`)
  const program = Buffer.from(convertBits(data.slice(1, -6), 5, 8))
  return Buffer.concat([Buffer.from([version === 0 ? 0 : 0x50 + version, program.length]), program])
}

// --- transactions ---
function varint(n) {
  if (n < 0xfd) return Buffer.from([n])
  const b = Buffer.alloc(n <= 0xffff ? 3 : 5)
  b[0] = n <= 0xffff ? 0xfd : 0xfe
  n <= 0xffff ? b.writeUInt16LE(n, 1) : b.writeUInt32LE(n, 1)
  return b
}
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n >>> 0); return b }
const u64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b }

function serialize(tx, withWitness) {
  const segwit = withWitness && tx.vin.some(i => i.witness.length)
  const parts = [u32(tx.version)]
  if (segwit) parts.push(Buffer.from([0, 1]))
  parts.push(varint(tx.vin.length))
  for (const i of tx.vin) parts.push(Buffer.from(i.txid, 'hex').reverse(), u32(i.vout), varint(i.scriptSig.length), i.scriptSig, u32(i.sequence))
  parts.push(varint(tx.vout.length))
  for (const o of tx.vout) parts.push(u64(o.value), varint(o.script.length), o.script)
  if (segwit) for (const i of tx.vin) {
    parts.push(varint(i.witness.length))
    for (const w of i.witness) parts.push(varint(w.length), w)
  }
  parts.push(u32(tx.locktime))
  return Buffer.concat(parts)
}

export function parseTx(raw) {
  let at = 0
  const read = n => raw.subarray(at, (at += n))
  const readVar = () => {
    const first = raw[at++]
    if (first < 0xfd) return first
    if (first === 0xfd) return read(2).readUInt16LE()
    if (first === 0xfe) return read(4).readUInt32LE()
    return Number(read(8).readBigUInt64LE())
  }
  const version = read(4).readInt32LE()
  let segwit = false
  if (raw[at] === 0 && raw[at + 1] === 1) {
    segwit = true
    at += 2
  }
  const vin = Array.from({ length: readVar() }, () => {
    const txid = reverseHex(read(32))
    const vout = read(4).readUInt32LE()
    const scriptSig = Buffer.from(read(readVar()))
    return { txid, vout, scriptSig, sequence: read(4).readUInt32LE(), witness: [] }
  })
  const vout = Array.from({ length: readVar() }, () => {
    const value = Number(read(8).readBigUInt64LE())
    return { value, script: Buffer.from(read(readVar())) }
  })
  if (segwit) for (const i of vin) i.witness = Array.from({ length: readVar() }, () => Buffer.from(read(readVar())))
  const locktime = read(4).readUInt32LE()
  return { version, vin, vout, locktime }
}

const txidOf = tx => reverseHex(dsha(serialize(tx, false)))

/** Starts the chain's HTTP server on a free loopback port. */
export async function startChain() {
  const blocks = [{ hash: TESTNET4_GENESIS, height: 0, time: 1714777860, txids: [] }]
  const newHash = () => randomBytes(32).toString('hex')
  for (let h = 1; h <= START_HEIGHT; h++) blocks.push({ hash: newHash(), height: h, time: blocks[0].time + h * BLOCK_SECS, txids: [] })
  const txs = new Map() // txid -> { tx, raw, block: null | block }
  const byScript = new Map() // scripthash -> txids (oldest first)
  const spends = new Map() // `${txid}:${vout}` -> { txid, vin }
  let mempool = []

  const scripthash = script => sha256(script).toString('hex')
  const index = (txid, script) => {
    const key = scripthash(script)
    const list = byScript.get(key) ?? []
    if (!list.includes(txid)) list.push(txid)
    byScript.set(key, list)
  }
  function accept(tx) {
    const txid = txidOf(tx)
    if (txs.has(txid)) return txid
    for (const [n, i] of tx.vin.entries()) {
      const prev = txs.get(i.txid)
      if (!prev) throw new Error(`Spends an unknown output ${i.txid}:${i.vout}`)
      if (spends.has(`${i.txid}:${i.vout}`)) throw new Error(`Output already spent ${i.txid}:${i.vout}`)
      spends.set(`${i.txid}:${i.vout}`, { txid, vin: n })
      index(txid, prev.tx.vout[i.vout].script)
    }
    for (const o of tx.vout) index(txid, o.script)
    txs.set(txid, { tx, raw: serialize(tx, true), block: null })
    mempool.push(txid)
    return txid
  }
  const tip = () => blocks[blocks.length - 1]

  const status = entry => entry.block
    ? { confirmed: true, block_height: entry.block.height, block_hash: entry.block.hash, block_time: entry.block.time }
    : { confirmed: false }
  function txJson(txid) {
    const entry = txs.get(txid)
    const { tx, raw } = entry
    const base = serialize(tx, false).length
    const prevouts = tx.vin.map(i => txs.get(i.txid)?.tx.vout[i.vout])
    const inSats = prevouts.reduce((sum, p) => sum + (p?.value ?? 0), 0)
    const outSats = tx.vout.reduce((sum, o) => sum + o.value, 0)
    return {
      txid,
      version: tx.version,
      locktime: tx.locktime,
      vin: tx.vin.map((i, n) => ({
        txid: i.txid,
        vout: i.vout,
        prevout: prevouts[n] ? { value: prevouts[n].value, scriptpubkey: prevouts[n].script.toString('hex') } : null,
        scriptsig: i.scriptSig.toString('hex'),
        witness: i.witness.map(w => w.toString('hex')),
        sequence: i.sequence,
        is_coinbase: false
      })),
      vout: tx.vout.map(o => ({ value: o.value, scriptpubkey: o.script.toString('hex') })),
      size: raw.length,
      weight: base * 3 + raw.length,
      status: status(entry),
      fee: Math.max(0, inSats - outSats)
    }
  }
  const blockJson = b => ({
    id: b.hash,
    height: b.height,
    version: 0x20000000,
    timestamp: b.time,
    tx_count: b.txids.length + 1,
    size: 285,
    weight: 1140,
    merkle_root: '00'.repeat(32),
    previousblockhash: b.height ? blocks[b.height - 1].hash : null,
    mediantime: b.time,
    nonce: 0,
    bits: 486604799,
    difficulty: 1
  })
  // Newest first, confirmed history before mempool, the way Esplora pages it.
  function historyFor(key) {
    const list = (byScript.get(key) ?? []).map(id => txs.get(id))
    const unconfirmed = list.filter(e => !e.block)
    const confirmed = list.filter(e => e.block).sort((a, b) => b.block.height - a.block.height)
    return [...unconfirmed, ...confirmed].map(e => txidOf(e.tx))
  }

  // A coin from nowhere: an output this chain will let one transaction spend.
  function mint(value) {
    const tx = { version: 2, locktime: 0, vin: [], vout: [{ value, script: Buffer.from([0x51]) }] }
    // A unique coinbase-like parent so every minted coin has its own txid.
    tx.vout.push({ value: 0, script: Buffer.concat([Buffer.from([0x6a, 0x20]), randomBytes(32)]) })
    const txid = txidOf(tx)
    txs.set(txid, { tx, raw: serialize(tx, true), block: blocks[1] })
    return txid
  }

  const api = {
    url: '',
    height: () => tip().height,
    /** Pays `sats` to `address` in a new transaction; confirmed unless `confirm` is false. */
    fund(address, sats, { confirm = true } = {}) {
      const parent = mint(sats + 1000)
      const txid = accept({ version: 2, locktime: 0, vin: [{ txid: parent, vout: 0, scriptSig: Buffer.from([0x01, 0x51]), sequence: 0xfffffffd, witness: [] }], vout: [{ value: sats, script: scriptForAddress(address) }] })
      if (confirm) api.mine(1)
      return txid
    },
    /** Mines `count` blocks; the first takes every mempool transaction. */
    mine(count = 1) {
      for (let n = 0; n < count; n++) {
        const block = { hash: newHash(), height: tip().height + 1, time: tip().time + BLOCK_SECS, txids: mempool }
        for (const txid of mempool) txs.get(txid).block = block
        mempool = []
        blocks.push(block)
      }
      return tip().height
    },
    mempool: () => [...mempool],
    tx: txid => (txs.has(txid) ? txJson(txid) : null),
    /** Transactions that pay `address`, newest first. */
    paymentsTo(address) {
      const script = scriptForAddress(address)
      return historyFor(scripthash(script)).map(txJson).filter(t => t.vout.some(o => o.scriptpubkey === script.toString('hex')))
    },
    close: () => new Promise(r => server.close(r))
  }

  const server = createServer((req, res) => {
    const send = (code, body, type = 'application/json') => {
      res.writeHead(code, { 'Content-Type': type })
      res.end(typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body))
    }
    const path = new URL(req.url, 'http://chain').pathname.replace(/\/+$/, '') || '/'
    if (process.env.OB_CHAIN_LOG) console.log('chain', req.method, path)
    let m
    if (req.method === 'POST' && path === '/tx') {
      let body = ''
      req.on('data', c => (body += c))
      req.on('end', () => {
        try {
          send(200, accept(parseTx(Buffer.from(body.trim(), 'hex'))), 'text/plain')
        } catch (e) {
          send(400, `sendrawtransaction RPC error: ${e.message}`, 'text/plain')
        }
      })
      return
    }
    if (path === '/blocks/tip/height') return send(200, String(tip().height), 'text/plain')
    if (path === '/blocks/tip/hash') return send(200, tip().hash, 'text/plain')
    if ((m = path.match(/^\/block-height\/(\d+)$/))) {
      const b = blocks[Number(m[1])]
      return b ? send(200, b.hash, 'text/plain') : send(404, 'Block not found', 'text/plain')
    }
    if ((m = path.match(/^\/blocks(?:\/(\d+))?$/))) {
      const from = Math.min(m[1] === undefined ? tip().height : Number(m[1]), tip().height)
      return send(200, blocks.slice(Math.max(0, from - 9), from + 1).reverse().map(blockJson))
    }
    if ((m = path.match(/^\/block\/([0-9a-f]{64})\/status$/))) {
      const b = blocks.find(x => x.hash === m[1])
      return send(200, b ? { in_best_chain: true, height: b.height, next_best: blocks[b.height + 1]?.hash ?? null } : { in_best_chain: false, height: null, next_best: null })
    }
    if ((m = path.match(/^\/block\/([0-9a-f]{64})$/))) {
      const b = blocks.find(x => x.hash === m[1])
      return b ? send(200, blockJson(b)) : send(404, 'Block not found', 'text/plain')
    }
    if ((m = path.match(/^\/scripthash\/([0-9a-f]{64})\/txs(?:\/chain(?:\/([0-9a-f]{64}))?)?$/))) {
      let ids = historyFor(m[1])
      if (path.includes('/chain')) {
        ids = ids.filter(id => txs.get(id).block)
        if (m[2]) ids = ids.slice(ids.indexOf(m[2]) + 1)
        return send(200, ids.slice(0, 25).map(txJson))
      }
      const unconfirmed = ids.filter(id => !txs.get(id).block)
      const confirmed = ids.filter(id => txs.get(id).block).slice(0, 25)
      return send(200, [...unconfirmed, ...confirmed].map(txJson))
    }
    if ((m = path.match(/^\/scripthash\/([0-9a-f]{64})\/txs\/mempool$/)))
      return send(200, historyFor(m[1]).filter(id => !txs.get(id).block).map(txJson))
    if ((m = path.match(/^\/tx\/([0-9a-f]{64})(\/raw|\/hex|\/status|\/outspends|\/outspend\/(\d+))?$/))) {
      const entry = txs.get(m[1])
      if (!entry) return send(404, 'Transaction not found', 'text/plain')
      if (m[2] === '/raw') return send(200, entry.raw, 'application/octet-stream')
      if (m[2] === '/hex') return send(200, entry.raw.toString('hex'), 'text/plain')
      if (m[2] === '/status') return send(200, status(entry))
      const outspend = n => {
        const s = spends.get(`${m[1]}:${n}`)
        return s ? { spent: true, txid: s.txid, vin: s.vin, status: status(txs.get(s.txid)) } : { spent: false }
      }
      if (m[2] === '/outspends') return send(200, entry.tx.vout.map((_, n) => outspend(n)))
      if (m[3] !== undefined) return send(200, outspend(Number(m[3])))
      return send(200, txJson(m[1]))
    }
    if (path === '/fee-estimates') return send(200, { 1: 3.0, 2: 2.5, 3: 2.0, 6: 1.5, 12: 1.2, 25: 1.1, 144: 1.0, 504: 1.0, 1008: 1.0 })
    if (path === '/mempool') return send(200, { count: mempool.length, vsize: 0, total_fee: 0, fee_histogram: [] })
    console.warn(`chain.mjs: not simulated: ${req.method} ${path}`)
    send(404, `Not simulated: ${req.method} ${path}`, 'text/plain')
  })
  await new Promise(r => server.listen(0, '127.0.0.1', r))
  api.url = `http://127.0.0.1:${server.address().port}`
  return api
}
