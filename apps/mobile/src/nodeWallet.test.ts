import assert from 'node:assert/strict'
import { test } from 'node:test'

import { networkInfo, networkStatus } from './networkInfo.ts'

const txid = 'a'.repeat(64)

test('network info keeps the node privacy warning and chain backend', () => {
  const info = networkInfo({
    network: 'testnet4',
    confirmations: 1,
    publicChainServer: true,
    lightning: 'offer-only',
    silentPayments: 'code-only',
    clearnet: true,
    privacyWarning: 'This node reaches the chain server over clearnet, so they see your IP address.',
    chainBackend: {
      backend: 'compact-filters',
      privacy: 'private',
      reveals: 'A public Esplora server is not asked which addresses this wallet follows.',
      fallback: 'https://mempool.space/testnet4/api',
      fallbackActive: false,
      scannedHeight: 159000,
      syncing: false
    },
    onion: 'abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion'
  })
  assert.equal(info.clearnet, true)
  assert.equal(info.scannedHeight, 159000)
  assert.equal(info.syncing, false)
  assert.equal(info.onion?.endsWith('.onion'), true)
  assert.match(networkStatus(info), /Testnet4/)
  assert.match(networkStatus(info), /Chain height 159000/)
  assert.match(networkStatus(info), /Onion abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx\.onion/)
  assert.match(info.privacyWarning ?? '', /IP address/)
  assert.equal(info.chain?.privacy, 'private')
  assert.match(info.chain?.reveals ?? '', /not asked which addresses/)
  assert.equal(info.chain?.fallbackActive, false)
  assert.equal(info.chain?.fallback, 'https://mempool.space/testnet4/api')
})

test('a missing warning stays empty instead of being replaced with a claim about address queries', () => {
  const info = networkInfo({
    network: 'testnet4',
    confirmations: 1,
    publicChainServer: false,
    privacyWarning: null,
    chainBackend: null
  })
  assert.equal(info.privacyWarning, null)
  assert.equal(info.chain, null)
  assert.equal(info.clearnet, false)
  assert.equal(info.onion, null)
  assert.equal(info.scannedHeight, null)
  assert.match(networkStatus(info), /Onion not published/)
  assert.match(networkStatus(info), /Chain height unavailable/)
  assert.equal(info.chainFailed, false)
  assert.equal(JSON.stringify(info).includes('mempool.space'), false)
  assert.equal(JSON.stringify(info).includes(txid), false)
})

test('a chain error is a failure and syncing stays a separate state', () => {
  const failed = networkInfo({
    network: 'testnet4',
    confirmations: 1,
    publicChainServer: false,
    chainBackend: { backend: 'compact-filters', privacy: 'private', reveals: 'none', error: 'peer timed out', syncing: false }
  })
  assert.equal(failed.chainFailed, true)
  assert.match(networkStatus(failed), /Chain sync failed/)
  assert.equal(networkStatus(failed).includes('Chain syncing'), false)
  const syncing = networkInfo({
    network: 'testnet4',
    confirmations: 1,
    publicChainServer: false,
    chainBackend: { backend: 'compact-filters', privacy: 'private', reveals: 'none', error: 'peer timed out', syncing: true, scannedHeight: 12 }
  })
  assert.match(networkStatus(syncing), /Chain syncing · height 12/)
})
