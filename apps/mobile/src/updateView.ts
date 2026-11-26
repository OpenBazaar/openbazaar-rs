import { downloadProgressPercent, formatBytes, formatProgress, formatSpeed, type UpdateStatus } from './updater.ts'

// What the update banner, the Settings card, and the sidebar pill say for each
// status. Pure, so each wording and each choice of buttons is tested.

export type UpdateAction = 'install' | 'later' | 'cancel' | 'retry' | 'dismiss'

export type Progress = {
  /** Whole percent, or null when the size is not known (an indeterminate bar). */
  percent: number | null
  /** "42%", or null with no size. */
  percentLabel: string | null
  /** "18.4 / 43.8 MB · 1.2 MB/s over Tor", or "18.4 MB over Tor" with no size. */
  detail: string
}

export function downloadProgress(status: UpdateStatus): Progress {
  const percent = downloadProgressPercent(status.downloaded_bytes, status.total_bytes)
  const amount = status.total_bytes && status.total_bytes > 0 ? formatProgress(status.downloaded_bytes, status.total_bytes) : formatBytes(status.downloaded_bytes)
  const parts = [amount]
  if (status.total_bytes && status.bytes_per_sec) parts.push(formatSpeed(status.bytes_per_sec))
  const detail = `${parts.join(' · ')} over Tor`
  return { percent, percentLabel: percent === null ? null : `${percent}%`, detail }
}

export type BannerView =
  | { kind: 'available'; title: string; notes: string | null; warning: string | null; actions: UpdateAction[] }
  | { kind: 'downloading'; title: string; progress: Progress; actions: UpdateAction[] }
  | { kind: 'installing'; title: string; note: string; actions: UpdateAction[] }
  | { kind: 'error'; title: string; message: string; actions: UpdateAction[] }

/** Tells banner states apart, so one dismissed offer does not hide the next thing to say. */
export function dismissKey(status: UpdateStatus): string {
  return `${status.stage}|${status.available_version ?? ''}|${status.error ?? ''}`
}

const named = (status: UpdateStatus) => `OpenBazaar ${status.available_version ?? 'update'}`

/**
 * The banner for a status, or null when there is none to show: nothing to say,
 * or an offer or error the user put away. Never null while downloading or installing.
 */
export function bannerView(status: UpdateStatus | null, dismissed: boolean): BannerView | null {
  if (!status) return null
  switch (status.stage) {
    case 'downloading':
      return { kind: 'downloading', title: `Downloading ${named(status)}`, progress: downloadProgress(status), actions: ['cancel'] }
    case 'installing':
      return {
        kind: 'installing',
        title: `Verifying & installing ${named(status)}…`,
        note: 'OpenBazaar will restart on its own when this is done.',
        actions: []
      }
    case 'available':
      if (dismissed) return null
      return {
        kind: 'available',
        title: `${named(status)} is ready`,
        notes: status.release_notes,
        warning: status.location_problem,
        actions: status.location_problem ? ['later'] : ['install', 'later']
      }
    case 'error':
      if (dismissed) return null
      return { kind: 'error', title: 'The update did not finish', message: status.error ?? 'Something went wrong.', actions: ['retry', 'dismiss'] }
    default:
      return null
  }
}

/** Retry downloads when a version is known (the check worked, the download did not); otherwise it checks again. */
export function retryAction(status: UpdateStatus): 'install' | 'check' {
  return status.available_version ? 'install' : 'check'
}

/** The sidebar pill, or null when there is nothing to point at. */
export function pillText(status: UpdateStatus | null): string | null {
  if (!status) return null
  if (status.stage === 'available') return 'Update ready'
  if (status.stage === 'downloading') {
    const percent = downloadProgressPercent(status.downloaded_bytes, status.total_bytes)
    return percent === null ? 'Downloading…' : `Downloading ${percent}%`
  }
  return null
}

/** Whether the Check button works: not while the shell is busy, and not without an updater key. */
export function canCheckNow(status: UpdateStatus | null, busy: boolean): boolean {
  if (!status || !status.can_check || busy) return false
  return status.stage !== 'checking' && status.stage !== 'downloading' && status.stage !== 'installing'
}

export type SettingsSummary = {
  tone: 'ok' | 'busy' | 'down'
  text: string
  /** The progress bar, while downloading. */
  progress: Progress | null
  /** Release notes of an update that is ready. */
  notes: string | null
  /** A reason this copy cannot update itself, shown instead of the offer's buttons. */
  warning: string | null
  /** Buttons beside the summary. */
  actions: UpdateAction[]
}

/** The summary line of Settings › App updates. */
export function settingsSummary(status: UpdateStatus): SettingsSummary {
  const none = { progress: null, notes: null, warning: null, actions: [] as UpdateAction[] }
  if (!status.can_check) {
    return { ...none, tone: 'down', text: 'This build of OpenBazaar cannot update itself. Download new versions from the OpenBazaar releases page.' }
  }
  switch (status.stage) {
    case 'checking':
      return { ...none, tone: 'busy', text: 'Checking over Tor…' }
    case 'available':
      return {
        ...none,
        tone: 'ok',
        text: status.available_version ? `Update v${status.available_version} available` : 'Update available',
        notes: status.release_notes,
        warning: status.location_problem,
        actions: status.location_problem ? [] : ['install']
      }
    case 'downloading':
      return {
        ...none,
        tone: 'busy',
        text: status.available_version ? `Downloading v${status.available_version}…` : 'Downloading…',
        progress: downloadProgress(status),
        notes: status.release_notes,
        actions: ['cancel']
      }
    case 'installing':
      return { ...none, tone: 'busy', text: 'Verifying & installing… OpenBazaar will restart on its own.' }
    case 'error':
      return { ...none, tone: 'down', text: status.error ?? 'Something went wrong.', actions: ['retry'] }
    case 'up_to_date':
      return { ...none, tone: 'ok', text: 'Up to date' }
    default:
      return { ...none, tone: 'ok', text: status.error ? `Last check failed: ${status.error}` : 'Not checked yet' }
  }
}
