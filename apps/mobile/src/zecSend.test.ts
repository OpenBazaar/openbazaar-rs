import assert from 'node:assert/strict'
import { test } from 'node:test'

import { parseZecAmount, parseZecSendQuote, zecAddressProblem } from './zecSend.ts'

test('a ZEC amount is zatoshi, and a blank or oversized one is not', () => {
  assert.equal(parseZecAmount('0.1'), 10_000_000n)
  assert.equal(parseZecAmount('1'), 100_000_000n)
  assert.equal(parseZecAmount('1.00000001'), 100_000_001n)
  assert.equal(parseZecAmount('1.000000001'), null)
  assert.equal(parseZecAmount('0'), null)
  assert.equal(parseZecAmount(''), null)
})

test('only a testnet unified address is offered to the node', () => {
  assert.equal(zecAddressProblem('', 'testnet'), 'Enter the address to send to.')
  assert.equal(zecAddressProblem('u1mainnetaddress000', 'testnet'), 'That is not a Zcash testnet address.')
  assert.equal(zecAddressProblem('utest1exampleaddress', 'mainnet'), 'Sending Zcash from this wallet is testnet only.')
  assert.equal(zecAddressProblem('zcash:utest1exampleaddress?amount=1', 'testnet'), null)
})

test('a send quote keeps the testnet terms the node priced', () => {
  const quote = parseZecSendQuote({
    quote: 'ab',
    network: 'testnet',
    address: 'utest1exampleaddress',
    amountZatoshi: '10000000',
    feeZatoshi: '10000',
    feeCapZatoshi: '15000',
    totalZatoshi: '10010000',
    expiresInSecs: 60
  })
  assert.equal(quote.amount, 10_000_000n)
  assert.equal(quote.feeCap, 15_000n)
  assert.throws(() => parseZecSendQuote({ quote: 'ab', network: 'mainnet', address: 'u1', amountZatoshi: '1', feeZatoshi: '1', feeCapZatoshi: '1', totalZatoshi: '2' }), /does not send/)
})
