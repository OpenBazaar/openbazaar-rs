import assert from 'node:assert/strict'
import { afterEach, test } from 'node:test'

import {
  UPDATE_EVENT,
  cancelUpdateDownload,
  checkForUpdates,
  downloadProgressPercent,
  formatBytes,
  formatProgress,
  formatSpeed,
  getUpdateStatus,
  installAvailableUpdate,
  isDesktopUpdaterAvailable,
  listenForUpdateStatus,
  parseUpdateStatus,
  setAutomaticUpdates,
  type UpdateStatus
} from './updater.ts'

const MB = 1024 * 1024
const scope = globalThis as { __TAURI_INTERNALS__?: unknown }

const status = (patch: Partial<UpdateStatus> = {}): UpdateStatus => ({
  stage: 'idle',
  current_version: '0.1.0',
  available_version: null,
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

type Call = { command: string; args?: Record<string, unknown> }

/** A stand-in for the desktop shell: records every call and answers from `reply`. */
function shell(reply: (command: string) => unknown, extra: Record<string, unknown> = {}) {
  const calls: Call[] = []
  scope.__TAURI_INTERNALS__ = {
    invoke: async (command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args })
      const answer = reply(command)
      if (answer instanceof Error) throw answer
      return answer
    },
    ...extra
  }
  return calls
}

afterEach(() => {
  delete scope.__TAURI_INTERNALS__
})

test('sizes are said in one unit with one decimal', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(512), '512 B')
  assert.equal(formatBytes(1024), '1.0 KB')
  assert.equal(formatBytes(1536), '1.5 KB')
  assert.equal(formatBytes(18.4 * MB), '18.4 MB')
  assert.equal(formatBytes(3 * 1024 * MB), '3.0 GB')
  assert.equal(formatBytes(2 * 1024 ** 4), '2048.0 GB', 'nothing larger than GB')
  assert.equal(formatBytes(-5), '0 B')
  assert.equal(formatBytes(Number.NaN), '0 B')
})

test('speed adds a rate to the size', () => {
  assert.equal(formatSpeed(1.2 * MB), '1.2 MB/s')
  assert.equal(formatSpeed(0), '0 B/s')
  assert.equal(formatSpeed(900), '900 B/s')
})

test('a pair of sizes shares the unit of the total', () => {
  assert.equal(formatProgress(18.4 * MB, 43.8 * MB), '18.4 / 43.8 MB')
  assert.equal(formatProgress(0.9 * MB, 2 * MB), '0.9 / 2.0 MB')
  assert.equal(formatProgress(0, 512), '0 / 512 B')
})

test('percent is whole and capped, and absent without a size', () => {
  assert.equal(downloadProgressPercent(0, 200), 0)
  assert.equal(downloadProgressPercent(84, 200), 42)
  assert.equal(downloadProgressPercent(199, 200), 99, 'rounds down, never claims done early')
  assert.equal(downloadProgressPercent(250, 200), 100)
  assert.equal(downloadProgressPercent(10, 0), null)
  assert.equal(downloadProgressPercent(10, null), null)
  assert.equal(downloadProgressPercent(-10, 200), 0)
})

test('the status parser keeps the shell’s fields and refuses anything else', () => {
  const sent = status({ stage: 'downloading', available_version: '0.2.0', downloaded_bytes: 5, total_bytes: 10, bytes_per_sec: 3, checked_at: 1700000000 })
  assert.deepEqual(parseUpdateStatus(sent), sent)
  assert.equal(parseUpdateStatus(null), null)
  assert.equal(parseUpdateStatus({ stage: 'weird', current_version: '0.1.0' }), null)
  assert.equal(parseUpdateStatus({ stage: 'idle' }), null, 'a status names the running version')
  const sloppy = parseUpdateStatus({ stage: 'idle', current_version: '0.1.0', downloaded_bytes: 'lots', total_bytes: -1, automatic: 'yes' })!
  assert.deepEqual([sloppy.downloaded_bytes, sloppy.total_bytes, sloppy.automatic, sloppy.can_check], [0, null, false, false])
})

test('outside the desktop app every call answers null and changes nothing', async () => {
  assert.equal(isDesktopUpdaterAvailable(), false)
  assert.equal(await getUpdateStatus(), null)
  assert.equal(await checkForUpdates(), null)
  assert.equal(await installAvailableUpdate(), null)
  assert.equal(await cancelUpdateDownload(), null)
  assert.equal(await setAutomaticUpdates(false), null)
  const stop = await listenForUpdateStatus(() => assert.fail('nothing to hear'))
  assert.equal(typeof stop, 'function')
  stop()
})

test('each call goes to its command in the shell', async () => {
  const calls = shell(() => status({ stage: 'checking' }))
  assert.equal(isDesktopUpdaterAvailable(), true)
  assert.equal((await getUpdateStatus())?.stage, 'checking')
  await checkForUpdates()
  await installAvailableUpdate()
  await cancelUpdateDownload()
  await setAutomaticUpdates(false)
  assert.deepEqual(calls, [
    { command: 'update_status', args: undefined },
    { command: 'check_for_updates', args: undefined },
    { command: 'install_available_update', args: undefined },
    { command: 'cancel_update_download', args: undefined },
    { command: 'set_automatic_updates', args: { enabled: false } }
  ])
})

test('a reply that is not a status is null', async () => {
  shell(() => 'nope')
  assert.equal(await getUpdateStatus(), null)
})

test('a refusal from the shell arrives as an Error carrying its words', async () => {
  scope.__TAURI_INTERNALS__ = { invoke: () => Promise.reject('OpenBazaar is already checking for or installing an update.') }
  await assert.rejects(installAvailableUpdate(), /already checking for or installing/)
  scope.__TAURI_INTERNALS__ = { invoke: () => Promise.reject(undefined) }
  await assert.rejects(installAvailableUpdate(), /could not be started/)
})

test('listening registers for the shell’s event and passes on statuses', async () => {
  let handler: ((event: unknown) => void) | undefined
  const unregistered: number[] = []
  const calls = shell(command => (command === 'plugin:event|listen' ? 41 : null), {
    transformCallback: (callback: (event: unknown) => void) => {
      handler = callback
      return 7
    },
    unregisterCallback: (id: number) => unregistered.push(id)
  })
  const heard: UpdateStatus[] = []
  const stop = await listenForUpdateStatus(next => heard.push(next))
  assert.deepEqual(calls[0], { command: 'plugin:event|listen', args: { event: UPDATE_EVENT, target: { kind: 'Any' }, handler: 7 } })
  assert.equal(UPDATE_EVENT, 'ob://update-status')
  handler?.({ payload: status({ stage: 'available', available_version: '0.2.0' }) })
  handler?.({ payload: { stage: 'garbage' } })
  assert.deepEqual(heard.map(item => item.stage), ['available'], 'only real statuses are passed on')
  stop()
  assert.deepEqual(unregistered, [7])
  assert.deepEqual(calls[1], { command: 'plugin:event|unlisten', args: { event: UPDATE_EVENT, eventId: 41 } })
})

test('without event support listening is a no-op, and polling covers it', async () => {
  const calls = shell(() => null)
  const stop = await listenForUpdateStatus(() => assert.fail('nothing to hear'))
  stop()
  assert.deepEqual(calls, [])
})
