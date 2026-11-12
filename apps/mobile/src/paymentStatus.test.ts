import assert from 'node:assert/strict'
import { test } from 'node:test'

import { FEE_RESERVE_SATS, paymentStatusView, type PaymentStatusInput } from './paymentStatus.ts'

function view(overrides: Partial<PaymentStatusInput> = {}) {
  return paymentStatusView({
    network: 'testnet4',
    status: 'Awaiting payment',
    confirmations: 0,
    requiredConfirmations: 1,
    dueSats: 2_400,
    sending: false,
    payTxid: null,
    wallet: { spendableSats: 12_000, totalSats: 12_000 },
    ...overrides
  })
}

test('a paid order no longer says it is waiting', () => {
  const paid = view({ status: 'Paid', confirmations: 1, dueSats: 0, payTxid: 'ab' })
  assert.equal(paid.progress, null)
  assert.equal(paid.balance, null)
})

test('a paid order shows Confirmed instead of a stale count', () => {
  assert.equal(view({ status: 'Paid', confirmations: 1 }).confirmations, 'Confirmed')
  assert.equal(view({ status: 'Paid', confirmations: 4 }).confirmations, 'Confirmed')
})

test('fulfilled orders do not request another payment when local counters lag', () => {
  for (const status of ['Shipped', 'Completed']) {
    const result = view({ status, confirmations: 0, payTxid: 'ab', sending: true, wallet: { spendableSats: 0, totalSats: 0 } })
    assert.equal(result.progress, null)
    assert.equal(result.balance, null)
    assert.equal(result.confirmations, 'Local confirmation details unavailable')
    assert.equal(view({ status, confirmations: 4 }).confirmations, 'Confirmations: 4')
  }
})

test('confirmations before Paid show progress against the requirement', () => {
  assert.equal(view({ status: 'Payment seen', confirmations: 1, requiredConfirmations: 2 }).confirmations, 'Confirmations: 1 / 2')
  assert.equal(view({ requiredConfirmations: undefined }).confirmations, 'Confirmations: 0')
})

test('progress follows sending, sent, and seen', () => {
  assert.equal(view({ sending: true }).progress, 'Sending payment from your node wallet…')
  assert.equal(view({ payTxid: 'ab' }).progress, 'Payment sent. Waiting for the node to see it on the chain.')
  assert.equal(view({ status: 'Payment seen', payTxid: 'ab' }).progress, 'Payment seen. Waiting for 1 more confirmation.')
  assert.equal(
    view({ status: 'Payment seen', confirmations: 1, requiredConfirmations: 3 }).progress,
    'Payment seen. Waiting for 2 more confirmations.'
  )
  assert.equal(view({ status: 'Payment seen', requiredConfirmations: undefined }).progress, 'Payment seen. Waiting for confirmations.')
  assert.equal(view().progress, null)
})

test('coins that only need a confirmation are not called insufficient', () => {
  const note = view({ wallet: { spendableSats: 0, totalSats: 12_000 } }).balance
  assert.equal(note?.kind, 'pending')
  assert.equal(note?.message, '12,000 sats are waiting for a confirmation. You can pay once they confirm.')
})

test('the balance check keeps a fee reserve', () => {
  const reserve = FEE_RESERVE_SATS.testnet4
  assert.equal(view({ wallet: { spendableSats: 2_400 + reserve, totalSats: 2_400 + reserve } }).balance, null)
  const exact = view({ wallet: { spendableSats: 2_400, totalSats: 2_400 } }).balance
  assert.equal(exact?.kind, 'short')
  assert.equal(
    exact?.message,
    `Your wallet has 2,400 spendable sats. This payment needs about 2,900, including up to ${reserve} sats kept for the network fee.`
  )
})

test('mainnet keeps a larger reserve', () => {
  assert.ok(FEE_RESERVE_SATS.mainnet > FEE_RESERVE_SATS.testnet4)
  const note = view({ network: 'mainnet', wallet: { spendableSats: 5_000, totalSats: 5_000 } }).balance
  assert.equal(note?.kind, 'short')
})

test('a short wallet mentions coins still waiting to confirm', () => {
  const note = view({ wallet: { spendableSats: 1_000, totalSats: 1_500 } }).balance
  assert.equal(note?.kind, 'short')
  assert.match(note?.message ?? '', /500 more sats are waiting for a confirmation\.$/)
})

test('own unconfirmed change is spendable, so it is not counted twice', () => {
  assert.equal(view({ wallet: { spendableSats: 9_445, totalSats: 9_445 } }).balance, null)
})

test('no wallet or nothing due gives no balance note', () => {
  assert.equal(view({ wallet: null }).balance, null)
  assert.equal(view({ dueSats: 0, wallet: { spendableSats: 0, totalSats: 0 } }).balance, null)
})
