import { canCheckNow, settingsSummary } from '../updateView'
import { UpdateProgress } from './UpdateBanner'
import type { DesktopUpdater } from './useDesktopUpdater'

const TONE_CLASS = { ok: '', busy: 'is-loading', down: 'is-offline' } as const

const checkedAt = (seconds: number | null) => (seconds ? new Date(seconds * 1000).toLocaleString() : null)

/** Settings › App updates: the version, the automatic check, and an update in progress. */
export function UpdateSettings({ updater }: { updater: DesktopUpdater }) {
  const { status, busy, actionError } = updater
  const summary = status ? settingsSummary(status) : null
  const last = status ? checkedAt(status.checked_at) : null
  return <div className='ob-settings-page'>
    <header className='ob-settings-heading'>
      <h1>App updates</h1>
      <p>OpenBazaar looks for new versions over Tor and installs one only when it is signed by the OpenBazaar release key. Your wallet, store, orders, and escrows are kept.</p>
    </header>
    {!updater.available && <p className='ob-settings-notice'>Updates are handled by the desktop app. This page has no updater.</p>}
    {updater.available && !status && <p role='status'>Reading the update status…</p>}
    {status && summary && <section className='ob-settings-card ob-update-card' aria-labelledby='ob-update-title'>
      <header className='ob-settings-card-heading'>
        <h2 id='ob-update-title'>Version {status.current_version}</h2>
      </header>
      <div className='ob-update-summary' role='status'>
        <span className={`ob-status-dot ${TONE_CLASS[summary.tone]}`} aria-hidden='true'/>
        <strong>{summary.text}</strong>
      </div>
      {summary.progress && <div className='ob-update-progress'>
        <UpdateProgress progress={summary.progress} label='Download progress'/>
        <p className='ob-update-detail'>
          {summary.progress.percentLabel && <span>{summary.progress.percentLabel}</span>}
          <span>{summary.progress.detail}</span>
        </p>
      </div>}
      {summary.warning && <p className='ob-update-warning'>{summary.warning}</p>}
      {summary.notes && <details className='ob-update-notes' open>
        <summary>Release notes</summary>
        <p>{summary.notes}</p>
      </details>}
      <div className='ob-update-actions'>
        {summary.actions.includes('install') && <button type='button' className='ob-primary' disabled={busy} onClick={() => void updater.install()}>Download &amp; install</button>}
        {summary.actions.includes('retry') && <button type='button' className='ob-primary' disabled={busy} onClick={() => void updater.retry()}>Retry</button>}
        {summary.actions.includes('cancel') && <button type='button' className='ob-secondary' disabled={busy} onClick={() => void updater.cancel()}>Cancel</button>}
        <button type='button' className='ob-secondary' disabled={!canCheckNow(status, busy)} onClick={() => void updater.check()}>Check for updates</button>
      </div>
      {actionError && <p className='ob-form-error' role='alert'>{actionError}</p>}
      <label className='ob-check ob-update-auto'>
        <input type='checkbox' checked={status.automatic} disabled={!status.can_check || busy} onChange={event => void updater.toggleAutomatic(event.target.checked)}/>
        Check for updates automatically
      </label>
      {last && <p className='ob-settings-footnote'>Last checked {last}.</p>}
    </section>}
  </div>
}
