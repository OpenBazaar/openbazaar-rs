import assert from 'node:assert/strict'
import { test } from 'node:test'

import { lnBlockedReason, lnStoreKeys, parseLnInvoice, parseLnStatus, parseLnStores, parseLnTerms } from './lightning.ts'

const invoice = 'lntbs188u1p4w75evdp0facx2mjzv9axzctjy'

test('a Lightning order payment parses, and a bitcoin one is null', () => {
  const payment = parseLnInvoice({
    rail: 'ln',
    invoice,
    uri: `lightning:${invoice.toUpperCase()}`,
    paymentHash: 'cd'.repeat(32),
    amountMsat: 18_800_001,
    expiresAt: 1_794_072_892,
    jit: true,
    issuer: 'local',
    lspFeeMsat: 188_000
  })
  assert.deepEqual(payment, {
    invoice,
    uri: `lightning:${invoice.toUpperCase()}`,
    paymentHash: 'cd'.repeat(32),
    amountSats: 18_801,
    expiresAt: 1_794_072_892,
    jit: true,
    expired: false,
    local: true,
    lspFeeSats: 188
  })
  assert.equal(parseLnInvoice({ address: 'tb1q', amountSats: 5 }), null)
  assert.throws(() => parseLnInvoice({ rail: 'ln', invoice: 'not an invoice', uri: 'x', paymentHash: 'x', amountMsat: 1, expiresAt: 1 }))
})

test('status keeps the node values and an off node is plainly off', () => {
  assert.equal(parseLnStatus({ enabled: false }).enabled, false)
  const status = parseLnStatus({
    enabled: true,
    network: 'signet',
    running: true,
    keys: 'ready',
    lsp: { nodeId: '03ab', address: '44.219.111.31:39735', connected: true },
    lspNotice: 'The LSP sees the amount.',
    lspAcknowledged: false,
    reveals: ['Lightning peers see this machine’s IP address.'],
    nodeId: '02cd',
    balances: { lightningSats: 49_500, inboundSats: 5_000, onchainSats: 0, onchainSpendableSats: 0 },
    channels: [{ id: '1', peer: '03ab', capacitySats: 55_000, outboundSats: 49_500, inboundSats: 5_000, ready: true, usable: true, fundingTxid: null }]
  })
  assert.equal(status.network, 'signet')
  assert.equal(status.lsp?.connected, true)
  assert.equal(status.balances?.lightningSats, 49_500)
  assert.equal(status.channels[0].capacitySats, 55_000)
  assert.throws(() => parseLnStatus({ enabled: true, network: 'litecoin' }))
})

test('stores and terms keep what the node said', () => {
  const stores = parseLnStores({ enabled: true, lspAcknowledged: true, lspNotice: null, stores: [{ seller: 'Marlow Goods', own: false, acceptsLightning: true }] })
  assert.deepEqual(stores.stores, [{ seller: 'Marlow Goods', own: false, acceptsLightning: true }])
  const terms = parseLnTerms({ orderId: 'a1', network: 'signet', paymentHash: 'ef', amountSats: 18_800, feeCapSats: 188, expiresInSecs: 120 })
  assert.equal(terms.feeCapSats, 188)
  assert.throws(() => parseLnTerms({ orderId: 'a1', network: 'signet', amountSats: 1, feeCapSats: 1, expiresInSecs: 1 }))
})

test('a mainnet node offers no Lightning at checkout, even to a store whose record takes it', () => {
  const blocked = 'Lightning on mainnet is disabled until its connections run over Tor; it would reveal this node’s IP address.'
  const stores = [
    { key: 'aa', acceptsLightning: true },
    { key: 'bb', acceptsLightning: false }
  ]
  assert.deepEqual([...lnStoreKeys(stores, { enabled: true, running: true, network: 'signet' })], ['aa'])
  assert.deepEqual([...lnStoreKeys(stores, { enabled: false, running: false, network: null, blocked })], [])
  assert.deepEqual([...lnStoreKeys(stores, { enabled: true, running: true, network: 'bitcoin' })], [], 'a node that names mainnet Lightning')
  assert.equal(lnBlockedReason(undefined), null)

  const status = parseLnStatus({ enabled: false, blocked })
  assert.equal(status.enabled, false)
  assert.equal(status.blocked, blocked)
  assert.equal(parseLnStatus({ enabled: false }).blocked, null)
  const list = parseLnStores({ enabled: false, blocked })
  assert.equal(list.blocked, blocked)
  assert.equal(list.enabled, false)
})
