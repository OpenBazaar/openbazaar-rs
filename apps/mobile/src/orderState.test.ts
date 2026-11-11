import assert from 'node:assert/strict'
import { test } from 'node:test'

import { parseBuyerOrder } from './catalog.ts'
import {
  canOfferBitcoinPay,
  confirmationTarget,
  isPaid,
  moneroDue,
  ORDER_STATUS,
  orderPollingDone,
  ordersSettled,
  payKind,
  zcashDue
} from './orderState.ts'

const bitcoin = {
  settlement: 'direct',
  status: ORDER_STATUS.awaitingPayment,
  payTxid: null,
  payment: { rail: 'btc' },
  escrow: null
}

test('a bitcoin order can be paid from the node wallet only while it is awaiting payment', () => {
  assert.equal(payKind(bitcoin), 'btc')
  assert.equal(canOfferBitcoinPay(bitcoin, 'btc', 1000), true)
  assert.equal(canOfferBitcoinPay({ ...bitcoin, status: ORDER_STATUS.paymentSeen }, 'btc', 1000), false)
  assert.equal(canOfferBitcoinPay({ ...bitcoin, payTxid: 'ab' }, 'btc', 1000), false)
  assert.equal(canOfferBitcoinPay(bitcoin, 'btc', 0), false)
  assert.equal(isPaid(ORDER_STATUS.paid), true)
  assert.equal(ordersSettled([]), true)
  assert.equal(ordersSettled([{ status: ORDER_STATUS.awaitingPayment }]), false)
  assert.equal(ordersSettled([{ status: ORDER_STATUS.paid }]), true)
})

test('a remote order keeps polling until the seller finishes or declines it', () => {
  const remote = { delivered: false }
  assert.equal(orderPollingDone({ status: ORDER_STATUS.paid, remote }), false)
  assert.equal(orderPollingDone({ status: ORDER_STATUS.completed, remote }), true)
  assert.equal(orderPollingDone({ status: ORDER_STATUS.declined, remote }), true)
  assert.equal(ordersSettled([{ status: ORDER_STATUS.paid }, { status: ORDER_STATUS.completed, remote }]), true)
})

test('escrow status never offers the bitcoin pay button', () => {
  const escrow = { ...bitcoin, settlement: 'escrow', escrow: { stage: 'offered' }, status: ORDER_STATUS.awaitingFunding }
  assert.equal(payKind(escrow), 'escrow')
  assert.equal(canOfferBitcoinPay(escrow, payKind(escrow), 1000), false)
  assert.equal(payKind({ ...bitcoin, status: ORDER_STATUS.awaitingTerms }), 'escrow')
})

test('a Monero order keeps its subaddress and does not offer bitcoin', () => {
  const parsed = parseBuyerOrder({
    id: 'order-1',
    checkoutId: 'check-1',
    created: 1,
    seller: 'Marlow Goods',
    storeId: 'aa',
    totalSats: 12_000,
    settlement: 'direct',
    status: 'Awaiting payment',
    lines: [{ listingHash: 'bb', title: 'Cup', quantity: 1, priceSats: 12_000 }],
    payment: {
      rail: 'xmr',
      network: 'stagenet',
      subaddress: '7Bexample',
      uri: 'monero:7Bexample?tx_amount=0.04',
      amountPiconero: '40000000000',
      receivedPiconero: '0',
      overpaidPiconero: '0',
      confirmationsNeeded: 10
    }
  })
  assert.equal(parsed.payment?.rail, 'xmr')
  assert.equal(parsed.payment?.address, '')
  assert.equal(parsed.payment?.ln, null)
  const due = moneroDue(parsed.payment)
  assert.equal(due?.amountXmr, '0.04')
  assert.equal(due?.remainingXmr, '0.04')
  assert.equal(due?.awaiting, true)
  assert.equal(payKind(parsed), 'xmr')
  assert.equal(canOfferBitcoinPay(parsed, payKind(parsed), parsed.totalSats), false)
  assert.equal(confirmationTarget('xmr', due?.confirmationsNeeded, 1), 10)
  assert.equal(confirmationTarget('btc', 10, 3), 3)
})

test('a bitcoin receipt stays on the bitcoin rail when the node omits rail', () => {
  const parsed = parseBuyerOrder({
    id: 'order-2',
    created: 1,
    seller: 'Marlow Goods',
    storeId: 'aa',
    totalSats: 2400,
    settlement: 'direct',
    status: 'Payment seen',
    payment: { address: 'tb1qexample', network: 'testnet4', amountSats: 2400 }
  })
  assert.equal(parsed.payment?.rail, 'btc')
  assert.equal(parsed.payment?.monero, null)
  assert.equal(payKind(parsed), 'btc')
})

test('a Zcash order keeps its shielded address, needs 30 confirmations, and does not offer bitcoin', () => {
  const parsed = parseBuyerOrder({
    id: 'order-3',
    checkoutId: 'check-3',
    created: 1,
    seller: 'Marlow Goods',
    storeId: 'aa',
    totalSats: 18_000,
    settlement: 'direct',
    status: 'Awaiting payment',
    lines: [{ listingHash: 'bb', title: 'Paperback novel', quantity: 1, priceSats: 18_000 }],
    payment: {
      rail: 'zec',
      network: 'testnet',
      pool: 'ironwood',
      zcashAddress: 'utest1example',
      addressIndex: 4,
      uri: 'zcash:utest1example?amount=0.01463415',
      amountZatoshi: '1463415',
      amountZec: '0.01463415',
      receivedZatoshi: '463415',
      remainingZatoshi: '1000000',
      overpaidZatoshi: '0',
      satsPerZec: 1_229_360,
      priceSats: 18_000,
      confirmationsNeeded: 30
    }
  })
  assert.equal(parsed.payment?.rail, 'zec')
  assert.equal(parsed.payment?.address, '', 'no bitcoin address, so the bitcoin pay path never sees it')
  assert.equal(parsed.payment?.monero, null)
  assert.equal(parsed.payment?.zcash?.address, 'utest1example')
  const due = zcashDue(parsed.payment)
  assert.deepEqual(
    [due?.amountZec, due?.receivedZec, due?.remainingZec, due?.overpaidZec, due?.awaiting],
    ['0.01463415', '0.00463415', '0.01', null, true]
  )
  assert.equal(due?.uri, 'zcash:utest1example?amount=0.01', 'after part arrived, the QR asks for the rest')
  const fresh = zcashDue({ rail: 'zec', zcash: { ...parsed.payment!.zcash!, receivedZatoshi: '0' } })
  assert.equal(fresh?.uri, 'zcash:utest1example?amount=0.01463415', "nothing arrived yet: the node's own URI")
  assert.equal(payKind(parsed), 'zec')
  assert.equal(canOfferBitcoinPay(parsed, payKind(parsed), parsed.totalSats), false)
  assert.equal(confirmationTarget('zec', due?.confirmationsNeeded, 1), 30)
  assert.equal(confirmationTarget('zec', undefined, 1), 30, 'the Zcash default, not the bitcoin one')
  assert.equal(moneroDue(parsed.payment), null)

  const paid = zcashDue({ rail: 'zec', zcash: { ...parsed.payment!.zcash!, receivedZatoshi: '1463422', overpaidZatoshi: '7' } })
  assert.deepEqual([paid?.awaiting, paid?.remainingZec, paid?.overpaidZec], [false, '0', '0.00000007'])
  assert.equal(zcashDue({ rail: 'zec', zcash: null }), null, 'an unreadable Zcash payment is not guessed at')
  const unreadable = parseBuyerOrder({
    id: 'order-4',
    created: 1,
    seller: 'Marlow Goods',
    storeId: 'aa',
    totalSats: 1,
    settlement: 'direct',
    status: 'Awaiting payment',
    payment: { rail: 'zec', network: 'testnet4', zcashAddress: 'utest1', uri: 'zcash:utest1', amountZatoshi: '1' }
  })
  assert.equal(unreadable.payment?.zcash, null, 'testnet4 is the bitcoin network, not a Zcash one')
  assert.equal(payKind(unreadable), 'zec', 'still not offered as bitcoin')
})
