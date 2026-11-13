import assert from 'node:assert/strict'
import { test } from 'node:test'

import { networkInfo, networkStatus } from './networkInfo.ts'
import { connectionRows, privacyStatus, type ConnectionInfo } from './walletConnection.ts'

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
  assert.equal(networkStatus(info).includes('Onion'), false)
  assert.equal(networkStatus(info).includes('.onion'), false)
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
  assert.equal(networkStatus(info).includes('Onion'), false)
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

function walletNetwork(patch: Partial<ConnectionInfo>): ConnectionInfo {
  return {
    proxy: null,
    proxyReachable: null,
    chainBackend: null,
    ...patch
  }
}

test('connection rows name the source, the proxy result, and what a server sees', () => {
  const rows = connectionRows(walletNetwork({
    proxy: 'socks5h://127.0.0.1:9050',
    proxyReachable: true,
    chainBackend: {
      backend: 'esplora',
      privacy: 'public',
      peers: null,
      scannedHeight: 160251,
      syncing: false,
      error: null,
      fallbackActive: true
    }
  }))
  const text = rows.map(row => `${row.label}: ${row.value}`).join('\n')
  assert.match(text, /Chain data source: Public chain server/)
  assert.match(text, /Proxy: 127\.0\.0\.1:9050 answered/)
  assert.match(text, /Your addresses stay on this node/)
  assert.equal(text.includes('--proxy'), false)
  assert.equal(text.includes('not a verified connection'), false)
  assert.equal(text.includes('Public fallback active'), false)
  const filters = connectionRows(walletNetwork({
    proxy: 'socks5h://127.0.0.1:9050',
    proxyReachable: true,
    chainBackend: {
      backend: 'compact-filters',
      privacy: 'private',
      peers: null,
      scannedHeight: 160257,
      syncing: false,
      error: null,
      fallbackActive: false
    }
  })).find(row => row.label === 'Chain data source')
  assert.equal(filters?.value, 'Compact block filters.')
  const closed = connectionRows(walletNetwork({
    proxy: 'socks5h://127.0.0.1:9050',
    proxyReachable: false
  })).find(row => row.label === 'Proxy')
  assert.match(closed?.value ?? '', /did not answer/)
  assert.match(closed?.value ?? '', /Start Tor/)
})

test('the shop banner names Tor when the proxy did not answer', () => {
  const failed = walletNetwork({
    proxy: 'socks5h://127.0.0.1:9050',
    proxyReachable: false,
    chainBackend: {
      backend: 'compact-filters',
      privacy: 'private',
      peers: null,
      scannedHeight: null,
      syncing: true,
      error: 'Tor is not listening on 127.0.0.1:9050. Start Tor, then restart the node.',
      fallbackActive: false
    }
  })
  assert.equal(privacyStatus(failed), 'Tor is not running')
  assert.equal(privacyStatus(walletNetwork({ proxy: 'socks5h://127.0.0.1:9050', proxyReachable: true })), '')
  assert.equal(
    privacyStatus(walletNetwork({
      proxy: 'socks5h://127.0.0.1:9050',
      proxyReachable: true,
      chainBackend: {
        backend: 'compact-filters',
        privacy: 'private',
        peers: null,
        scannedHeight: null,
        syncing: false,
        error: 'peer timed out',
        fallbackActive: false
      }
    })),
    'Chain transport failed'
  )
})
