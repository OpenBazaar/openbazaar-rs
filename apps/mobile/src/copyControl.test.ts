import assert from 'node:assert/strict'
import { test } from 'node:test'
import { copyValueLabel } from './copyControl.ts'

test('copy buttons name the value they copy', () => {
  assert.equal(copyValueLabel('Receive address'), 'Copy receive address')
  assert.equal(copyValueLabel('Transaction ID'), 'Copy transaction ID')
  assert.equal(copyValueLabel('Payment address'), 'Copy payment address')
  assert.equal(copyValueLabel('Monero subaddress'), 'Copy monero subaddress')
  assert.equal(copyValueLabel('Zcash address'), 'Copy zcash address')
})
