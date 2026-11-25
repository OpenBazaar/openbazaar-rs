import assert from 'node:assert/strict'
import { test } from 'node:test'

import { parseTorStatus, torFeatures, torIndicator, torSummary, waitingForTor } from './torStatus.ts'

const sent = (patch: Record<string, unknown>) =>
  parseTorStatus({ mode: 'builtin', state: 'connected', progress: 100, error: null, onionServices: [], ...patch })!

test('the indicator is green, amber, or red, and says which Tor', () => {
  assert.deepEqual(torIndicator(sent({})), { tone: 'ok', label: 'Tor connected (built-in)', title: 'Tor is connected and can build circuits.' })
  const connecting = torIndicator(sent({ state: 'connecting', progress: 42 }))!
  assert.equal(connecting.tone, 'busy')
  assert.equal(connecting.label, 'Tor connecting… 42%')
  const degraded = torIndicator(sent({ state: 'degraded', error: 'too many preemptive onion service circuits failed' }))!
  assert.equal(degraded.tone, 'busy')
  assert.equal(degraded.label, 'Tor having trouble (built-in)')
  assert.match(degraded.title, /preemptive onion service circuits/)
  assert.match(torSummary(sent({ state: 'degraded', error: 'guards failing' })), /having trouble: guards failing/)
  assert.equal(waitingForTor(sent({ state: 'degraded' })), true)
  const failed = torIndicator(sent({ mode: 'system', state: 'failed', progress: null, error: 'the Tor proxy at 127.0.0.1:9050 does not answer' }))!
  assert.equal(failed.tone, 'down')
  assert.equal(failed.label, 'Tor not connected (system)')
  assert.match(failed.title, /9050 does not answer/)
  assert.equal(torIndicator(sent({ mode: 'none', state: 'off' })), null, 'no Tor, no indicator')
  assert.equal(torIndicator(null), null)
})

test('the status parser keeps only known words and onion services', () => {
  const tor = parseTorStatus({ mode: 'weird', state: 'nope', progress: 140, onionServices: [{ name: 'store', published: true }, { published: 'yes' }] })!
  assert.deepEqual(tor, { mode: 'none', state: 'off', progress: null, error: null, onionServices: [{ name: 'store', published: true }, { name: 'unknown', published: false }] })
  assert.equal(parseTorStatus(undefined), null)
  assert.match(torSummary(sent({ state: 'failed', error: 'no internet' })), /built-in Tor is not connected: no internet/)
})

test('features point at Tor while it is down, and say their own state once it is up', () => {
  const network = {
    proxy: 'socks5h://127.0.0.1:5555',
    liveRates: true,
    chainBackend: { backend: 'compact-filters', peers: 2, scannedHeight: 161696, syncing: false, fallbackActive: false },
    xmrBackend: { backend: 'remote-node', proxied: true, connecting: true, error: null },
    lightning: { enabled: true, running: true },
    zecBackend: { enabled: true, proxied: true, active: 'https://testnet.zec.rocks:443', error: null }
  }
  const down = sent({ state: 'connecting', progress: 10 })
  assert.equal(waitingForTor(down), true)
  const waiting = torFeatures(network, down)
  assert.deepEqual(waiting.map(feature => feature.name), ['Bitcoin peers', 'Monero', 'Lightning', 'Zcash indexer', 'Exchange rates', 'Package tracking'])
  assert.ok(waiting.every(feature => feature.state === 'Waiting for Tor'))
  const up = Object.fromEntries(torFeatures(network, sent({})).map(feature => [feature.name, feature.state]))
  assert.equal(up['Bitcoin peers'], `2 peers, scanned to block ${(161696).toLocaleString()}`)
  assert.equal(up.Monero, 'Connecting to a Monero node')
  assert.equal(up['Zcash indexer'], 'Connected')
  assert.equal(up.Lightning, 'Running')
})
