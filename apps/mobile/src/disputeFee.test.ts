import assert from 'node:assert/strict'
import { test } from 'node:test'

import { parseBuyerOrder } from './catalog.ts'
import { disputeFeeText, filingFeeNote, parseDisputeFee, parseDisputeFeeQuote, verifiedDenial, verifiedReceipt } from './disputeFee.ts'

// Signed by crates/protocol with the secret [7; 32]; see records.test.ts.
const MODERATOR = '989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f'
const RECEIPT = '4f42465201989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f11111111111111111111111111111111111111111111111111111111111111112222222222222222222222222222222222222222222222222222222222222222014444444444444444444444444444444444444444444444444444444444444444c409000000000000780000002c0100008834a2eceb5b6aa924f38434d6d49b74aee0309b5e33e72c1baebe2ea192e66906eb559d74965c81a9ce6b67ba4dd500217f0c715aaf7aa889e8ebfdd8a5ca4c'
const DENIAL = '4f42444401989c0b76cb563971fdc9bef31ec06c3560f3249d6ee9e5d83c57625596e05f6f11111111111111111111111111111111111111111111111111111111111111110300000000000000000000000000000000000000000000000000000000000000005e2f1a15d12c2c4e53d01057269ba75c0b8a7ea98556f7991e542331ee990872dbf724ebbdb39a5279094af1e30d89571f940684ba513f7e013f4ca1f91ec64b'

const fee = (extra: Record<string, unknown> = {}) => parseDisputeFee({ state: 'requested', amountSats: 2500, validUntilHeight: 180, dueHeight: 300, expired: false, lightning: true, rail: null, txid: null, receipt: null, denial: null, ...extra })!

test('a dispute fee parses as the node sends it, and an unknown state is no fee', () => {
  assert.deepEqual(fee(), { state: 'requested', amountSats: 2500, validUntilHeight: 180, dueHeight: 300, expired: false, lightning: true, rail: null, txid: null, receipt: null, denial: null })
  assert.equal(parseDisputeFee(null), null)
  assert.equal(parseDisputeFee({ state: 'refunded' }), null)
  assert.equal(parseDisputeFee({ state: 'paid', amountSats: -1 })?.amountSats, null)
})

test('the order carries the fee kind and the filer fee', () => {
  const order = (status: string, escrow: Record<string, unknown>) =>
    parseBuyerOrder({ id: 'order-1', created: 1, seller: 'Marlow Goods', storeId: 'aa', totalSats: 12_000, settlement: 'escrow', status, lines: [{ listingHash: 'bb', title: 'Cup', quantity: 1, priceSats: 12_000 }], escrow }).escrow
  const filed = order('Disputed', { stage: 'disputed', feeKind: 'filing', moderatorFeeSats: 2500, disputeFee: { state: 'awaiting-request' } })
  assert.equal(filed?.feeKind, 'filing')
  assert.equal(filed?.disputeFee?.state, 'awaiting-request')
  const old = order('Funded', { stage: 'funded', feeKind: 'ruling' })
  assert.equal(old?.feeKind, 'ruling')
  assert.equal(old?.disputeFee, undefined)
})

test('a fee quote names its rail, and an incomplete one is an error', () => {
  assert.deepEqual(parseDisputeFeeQuote({ quote: 'q', rail: 'onchain', amountSats: 2500, feeCapSats: 900, feeSats: 300, totalSats: 2800, address: 'tb1q', expiresInSecs: 120 }), { quote: 'q', rail: 'onchain', amountSats: 2500, feeCapSats: 900, feeSats: 300, totalSats: 2800, address: 'tb1q', expiresInSecs: 120 })
  assert.equal(parseDisputeFeeQuote({ quote: 'q', rail: 'ln', amountSats: 2500, feeCapSats: 25 }).feeSats, null)
  assert.throws(() => parseDisputeFeeQuote({ quote: 'q', rail: 'xmr', amountSats: 1, feeCapSats: 1 }))
  assert.throws(() => parseDisputeFeeQuote({ rail: 'ln', amountSats: 1, feeCapSats: 1 }))
})

test('the receipt and the denial count only when the moderator in the terms signed them', () => {
  assert.equal(verifiedReceipt(fee({ state: 'paid', receipt: RECEIPT }), MODERATOR)?.receivedSats, 2500)
  assert.equal(verifiedReceipt(fee({ state: 'paid', receipt: RECEIPT }), '8f'.repeat(32)), null)
  assert.equal(verifiedDenial(fee({ state: 'denied', denial: DENIAL }), MODERATOR), true)
  assert.equal(verifiedDenial(fee({ state: 'denied', denial: RECEIPT }), MODERATOR), false)
})

test('the filer reads what its fee is waiting on', () => {
  assert.match(disputeFeeText(fee()), /asks 2,500 sats/)
  assert.match(disputeFeeText(fee({ expired: true })), /lapsed/)
  assert.match(disputeFeeText(fee({ state: 'paid' }), 300), /due by block 300/)
  assert.match(disputeFeeText(fee({ state: 'paid' }), 301), /overdue.*not refunded/)
  assert.match(disputeFeeText(fee({ state: 'denied' })), /final/)
  assert.match(filingFeeNote('filing', 2500), /^Opening a dispute costs 2,500 sats/)
  assert.equal(filingFeeNote('filing', 0), 'This moderator charges no filing fee.')
  assert.match(filingFeeNote('ruling', 1000), /only if it rules/)
  assert.equal(filingFeeNote(undefined, 0), '')
})
