// In-app updates, as the desktop shell offers them (apps/desktop/src-tauri/src/updates.rs).
// The shell checks, downloads, and installs; this page only asks it to and
// shows its status. Outside the desktop app there is no shell, so every call
// here answers null and changes nothing.

export type UpdateStage = 'idle' | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'installing' | 'error'

/** The shell's status, as it sends it (field names are the shell's). */
export type UpdateStatus = {
  stage: UpdateStage
  current_version: string
  available_version: string | null
  release_notes: string | null
  downloaded_bytes: number
  total_bytes: number | null
  bytes_per_sec: number | null
  /** Unix seconds of the last check that reached the manifest. */
  checked_at: number | null
  error: string | null
  automatic: boolean
  /** Why this copy cannot replace itself where it runs. */
  location_problem: string | null
  /** False in a build with no updater key. */
  can_check: boolean
}

/** The event the shell sends, with an UpdateStatus, whenever the status changes. */
export const UPDATE_EVENT = 'ob://update-status'

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>
type Internals = {
  invoke?: Invoke
  transformCallback?: (callback: (event: unknown) => void, once?: boolean) => number
  unregisterCallback?: (id: number) => void
}

const STAGES: readonly UpdateStage[] = ['idle', 'checking', 'up_to_date', 'available', 'downloading', 'installing', 'error']

function shellInternals(): Internals | null {
  const internals = (globalThis as { __TAURI_INTERNALS__?: Internals }).__TAURI_INTERNALS__
  return typeof internals?.invoke === 'function' ? internals : null
}

/** True inside the desktop app. */
export function isDesktopUpdaterAvailable(): boolean {
  return shellInternals() !== null
}

const record = (value: unknown): Record<string, unknown> => (value && typeof value === 'object' ? (value as Record<string, unknown>) : {})
const text = (value: unknown): string | null => (typeof value === 'string' ? value : null)
const count = (value: unknown): number | null => (typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null)

/** The status from the shell, or null when it is not one. */
export function parseUpdateStatus(value: unknown): UpdateStatus | null {
  const body = record(value)
  const stage = STAGES.find(known => known === body.stage)
  const current = text(body.current_version)
  if (!stage || current === null) return null
  return {
    stage,
    current_version: current,
    available_version: text(body.available_version),
    release_notes: text(body.release_notes),
    downloaded_bytes: count(body.downloaded_bytes) ?? 0,
    total_bytes: count(body.total_bytes),
    bytes_per_sec: count(body.bytes_per_sec),
    checked_at: count(body.checked_at),
    error: text(body.error),
    automatic: body.automatic === true,
    location_problem: text(body.location_problem),
    can_check: body.can_check === true
  }
}

/** What the shell refused with, as an Error (the shell rejects with a bare string). */
function refusal(err: unknown): Error {
  if (err instanceof Error) return err
  return new Error(typeof err === 'string' && err ? err : 'The update could not be started.')
}

async function ask(command: string, args?: Record<string, unknown>): Promise<UpdateStatus | null> {
  const internals = shellInternals()
  if (!internals?.invoke) return null
  try {
    return parseUpdateStatus(await internals.invoke(command, args))
  } catch (err) {
    throw refusal(err)
  }
}

/** The shell's status right now. null outside the desktop app. */
export function getUpdateStatus(): Promise<UpdateStatus | null> {
  return ask('update_status')
}

/** Start a check over Tor. Answers at once, already "checking". */
export function checkForUpdates(): Promise<UpdateStatus | null> {
  return ask('check_for_updates')
}

/** Download and install the update the last check found. Rejects with the shell's reason when it cannot start. */
export function installAvailableUpdate(): Promise<UpdateStatus | null> {
  return ask('install_available_update')
}

export function cancelUpdateDownload(): Promise<UpdateStatus | null> {
  return ask('cancel_update_download')
}

export function setAutomaticUpdates(enabled: boolean): Promise<UpdateStatus | null> {
  return ask('set_automatic_updates', { enabled })
}

/**
 * Hear of every status change the shell makes. Resolves to a function that
 * stops listening; where events are not available it does nothing, and the
 * caller's polling covers it.
 */
export async function listenForUpdateStatus(onStatus: (status: UpdateStatus) => void): Promise<() => void> {
  const internals = shellInternals()
  const { invoke, transformCallback } = internals ?? {}
  if (!invoke || !transformCallback) return () => {}
  try {
    const handler = transformCallback(event => {
      const status = parseUpdateStatus(record(event).payload)
      if (status) onStatus(status)
    })
    const eventId = await invoke('plugin:event|listen', { event: UPDATE_EVENT, target: { kind: 'Any' }, handler })
    return () => {
      internals?.unregisterCallback?.(handler)
      void Promise.resolve(invoke('plugin:event|unlisten', { event: UPDATE_EVENT, eventId })).catch(() => {})
    }
  } catch {
    return () => {}
  }
}

// How sizes and speeds are said. The shell says them the same way in the menu
// bar (updates.rs): binary units, one decimal, one unit for a pair.

const UNITS = ['B', 'KB', 'MB', 'GB'] as const

function unitFor(bytes: number): number {
  let unit = 0
  let size = Math.floor(bytes)
  while (size >= 1024 && unit < UNITS.length - 1) {
    size = Math.floor(size / 1024)
    unit += 1
  }
  return unit
}

const scaled = (bytes: number, unit: number) => (unit === 0 ? String(Math.floor(bytes)) : (bytes / 1024 ** unit).toFixed(1))
const whole = (bytes: number) => (Number.isFinite(bytes) && bytes > 0 ? bytes : 0)

/** "18.4 MB" */
export function formatBytes(bytes: number): string {
  const size = whole(bytes)
  const unit = unitFor(size)
  return `${scaled(size, unit)} ${UNITS[unit]}`
}

/** "1.2 MB/s" */
export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`
}

/** "18.4 / 43.8 MB": both in the unit of the total. */
export function formatProgress(downloaded: number, total: number): string {
  const size = whole(total)
  const unit = unitFor(size)
  return `${scaled(whole(downloaded), unit)} / ${scaled(size, unit)} ${UNITS[unit]}`
}

/** Whole percent, never above 100. null when the size is not known. */
export function downloadProgressPercent(downloaded: number, total: number | null): number | null {
  if (total === null || !Number.isFinite(total) || total <= 0) return null
  return Math.min(100, Math.max(0, Math.floor((whole(downloaded) * 100) / total)))
}
