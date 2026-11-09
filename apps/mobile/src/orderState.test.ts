import assert from 'node:assert/strict'
import { test } from 'node:test'

import { parseBuyerOrder } from './catalog.ts'
import {
  canOfferBitcoinPay,
  confirmationTarget,
  isPaid,
  moneroDue,
  ORDER_STATUS,
  ordersSettled,
  payKind
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
