import assert from 'node:assert/strict'
import { test } from 'node:test'

import { bannerView, canCheckNow, dismissKey, downloadProgress, pillText, retryAction, settingsSummary } from './updateView.ts'
import type { UpdateStatus } from './updater.ts'

const MB = 1024 * 1024

const status = (patch: Partial<UpdateStatus> = {}): UpdateStatus => ({
  stage: 'idle',
  current_version: '0.1.0',
  available_version: '0.2.0',
  release_notes: null,
  downloaded_bytes: 0,
  total_bytes: null,
  bytes_per_sec: null,
  checked_at: null,
  error: null,
  automatic: true,
  location_problem: null,
  can_check: true,
  ...patch
})

const downloading = (patch: Partial<UpdateStatus> = {}) =>
  status({ stage: 'downloading', downloaded_bytes: 18.4 * MB, total_bytes: 43.8 * MB, bytes_per_sec: 1.2 * MB, ...patch })

test('a known size gives a determinate bar, the amounts, and the speed over Tor', () => {
  assert.deepEqual(downloadProgress(downloading()), { percent: 42, percentLabel: '42%', detail: '18.4 / 43.8 MB · 1.2 MB/s over Tor' })
})

test('a known size with no speed yet leaves the speed out', () => {
  assert.equal(downloadProgress(downloading({ bytes_per_sec: null })).detail, '18.4 / 43.8 MB over Tor')
})

test('an unknown size gives an indeterminate bar and only what has come in', () => {
  assert.deepEqual(downloadProgress(downloading({ total_bytes: null })), { percent: null, percentLabel: null, detail: '18.4 MB over Tor' })
})

test('nothing is shown for the states that have nothing to say', () => {
  assert.equal(bannerView(null, false), null)
  for (const stage of ['idle', 'checking', 'up_to_date'] as const) assert.equal(bannerView(status({ stage }), false), null, stage)
})

test('an update that is ready offers Download & install and Later, with its notes', () => {
  const view = bannerView(status({ stage: 'available', release_notes: 'Faster search.' }), false)
  assert.deepEqual(view, { kind: 'available', title: 'OpenBazaar 0.2.0 is ready', notes: 'Faster search.', warning: null, actions: ['install', 'later'] })
})

test('an update that cannot replace this copy warns instead of offering the download', () => {
  const view = bannerView(status({ stage: 'available', location_problem: 'Move the app to Applications first.' }), false)
  assert.equal(view?.kind, 'available')
  assert.equal(view?.kind === 'available' && view.warning, 'Move the app to Applications first.')
  assert.deepEqual(view?.actions, ['later'])
})

test('Later puts the offer away', () => {
  assert.equal(bannerView(status({ stage: 'available' }), true), null)
})

test('downloading shows the bar and Cancel, and cannot be put away', () => {
  const view = bannerView(downloading(), true)
  assert.equal(view?.kind, 'downloading')
  assert.equal(view?.title, 'Downloading OpenBazaar 0.2.0')
  assert.deepEqual(view?.actions, ['cancel'])
  assert.equal(view?.kind === 'downloading' && view.progress.percent, 42)
  const unknown = bannerView(downloading({ total_bytes: null }), false)
  assert.equal(unknown?.kind === 'downloading' && unknown.progress.percent, null)
})

test('installing says it is verifying, shows no buttons, and cannot be put away', () => {
  const view = bannerView(status({ stage: 'installing' }), true)
  assert.deepEqual(view, {
    kind: 'installing',
    title: 'Verifying & installing OpenBazaar 0.2.0…',
    note: 'OpenBazaar will restart on its own when this is done.',
    actions: []
  })
})

test('an error shows its message with Retry and Dismiss until dismissed', () => {
  const failed = status({ stage: 'error', error: 'connection reset' })
  assert.deepEqual(bannerView(failed, false), { kind: 'error', title: 'The update did not finish', message: 'connection reset', actions: ['retry', 'dismiss'] })
  assert.equal(bannerView(failed, true), null)
})

test('Retry downloads when the version is known and checks again when it is not', () => {
  assert.equal(retryAction(status({ stage: 'error', available_version: '0.2.0', error: 'x' })), 'install')
  assert.equal(retryAction(status({ stage: 'error', available_version: null, error: 'x' })), 'check')
})

test('a dismissed offer does not hide the next thing the shell says', () => {
  const offer = dismissKey(status({ stage: 'available' }))
  assert.notEqual(offer, dismissKey(status({ stage: 'available', available_version: '0.3.0' })))
  assert.notEqual(offer, dismissKey(status({ stage: 'error', error: 'x' })))
  assert.equal(offer, dismissKey(status({ stage: 'available', downloaded_bytes: 5 })), 'the same offer is the same key')
})

test('the sidebar pill points at an update that is ready or coming in', () => {
  assert.equal(pillText(null), null)
  assert.equal(pillText(status({ stage: 'up_to_date' })), null)
  assert.equal(pillText(status({ stage: 'available' })), 'Update ready')
  assert.equal(pillText(downloading()), 'Downloading 42%')
  assert.equal(pillText(downloading({ total_bytes: null })), 'Downloading…')
  assert.equal(pillText(status({ stage: 'error', error: 'x' })), null)
})

test('Check for updates works unless the shell is busy or has no key', () => {
  assert.equal(canCheckNow(null, false), false)
  assert.equal(canCheckNow(status({ stage: 'up_to_date' }), false), true)
  assert.equal(canCheckNow(status({ stage: 'error' }), false), true)
  assert.equal(canCheckNow(status({ stage: 'up_to_date' }), true), false, 'a request is in flight')
  for (const stage of ['checking', 'downloading', 'installing'] as const) assert.equal(canCheckNow(status({ stage }), false), false, stage)
  assert.equal(canCheckNow(status({ can_check: false }), false), false)
})

test('Settings says each state in a line', () => {
  assert.equal(settingsSummary(status({ stage: 'up_to_date' })).text, 'Up to date')
  assert.equal(settingsSummary(status({ stage: 'checking' })).text, 'Checking over Tor…')
  assert.equal(settingsSummary(status({ stage: 'installing' })).tone, 'busy')
  assert.match(settingsSummary(status({ stage: 'idle', error: 'no Tor' })).text, /Last check failed: no Tor/)
  assert.equal(settingsSummary(status({ stage: 'idle' })).text, 'Not checked yet')
  assert.match(settingsSummary(status({ can_check: false })).text, /cannot update itself/)
})

test('Settings shows an available update with notes and the install button', () => {
  const summary = settingsSummary(status({ stage: 'available', release_notes: 'Faster search.' }))
  assert.equal(summary.text, 'Update v0.2.0 available')
  assert.equal(summary.notes, 'Faster search.')
  assert.deepEqual(summary.actions, ['install'])
  const stuck = settingsSummary(status({ stage: 'available', location_problem: 'Move the app first.' }))
  assert.equal(stuck.warning, 'Move the app first.')
  assert.deepEqual(stuck.actions, [], 'no download that would fail at the end')
})

test('Settings shows live progress and Cancel while downloading, determinate or not', () => {
  const known = settingsSummary(downloading())
  assert.equal(known.text, 'Downloading v0.2.0…')
  assert.equal(known.progress?.percent, 42)
  assert.equal(known.progress?.detail, '18.4 / 43.8 MB · 1.2 MB/s over Tor')
  assert.deepEqual(known.actions, ['cancel'])
  const unknown = settingsSummary(downloading({ total_bytes: null }))
  assert.equal(unknown.progress?.percent, null)
  assert.equal(unknown.progress?.detail, '18.4 MB over Tor')
  assert.equal(settingsSummary(downloading({ release_notes: 'Notes here.' })).notes, 'Notes here.')
})

test('Settings shows an error with Retry', () => {
  const summary = settingsSummary(status({ stage: 'error', error: 'connection reset' }))
  assert.equal(summary.text, 'connection reset')
  assert.equal(summary.tone, 'down')
  assert.deepEqual(summary.actions, ['retry'])
})
