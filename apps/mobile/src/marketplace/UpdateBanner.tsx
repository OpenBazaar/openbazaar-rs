import type { Progress } from '../updateView'
import { bannerView, pillText, type BannerView } from '../updateView'
import type { DesktopUpdater } from './useDesktopUpdater'

/** A determinate bar with a percentage, or an indeterminate one when the size is not known. */
export function UpdateProgress({ progress, label, indeterminate = false }: { progress: Progress | null; label: string; indeterminate?: boolean }) {
  const percent = indeterminate ? null : (progress?.percent ?? null)
  return <div
    className={`ob-update-bar${percent === null ? ' is-indeterminate' : ''}`}
    role='progressbar'
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={percent ?? undefined}
    aria-valuetext={percent === null ? undefined : `${percent}%`}
  >
    <span style={percent === null ? undefined : { width: `${percent}%` }}/>
  </div>
}

function Body({ view, updater }: { view: BannerView; updater: DesktopUpdater }) {
  const { busy } = updater
  const has = (action: BannerView['actions'][number]) => view.actions.includes(action)
  return <>
    <div className='ob-update-copy'>
      <strong>{view.title}</strong>
      {view.kind === 'available' && view.warning && <p>{view.warning}</p>}
      {view.kind === 'available' && view.notes && <details className='ob-update-notes'>
        <summary>Release notes</summary>
        <p>{view.notes}</p>
      </details>}
      {view.kind === 'downloading' && <>
        <UpdateProgress progress={view.progress} label={view.title}/>
        <p className='ob-update-detail'>
          {view.progress.percentLabel && <span>{view.progress.percentLabel}</span>}
          <span>{view.progress.detail}</span>
        </p>
      </>}
      {view.kind === 'installing' && <>
        <UpdateProgress progress={null} label={view.title} indeterminate/>
        <p className='ob-update-detail'><span>{view.note}</span></p>
      </>}
      {view.kind === 'error' && <p>{view.message}</p>}
    </div>
    {view.actions.length > 0 && <div className='ob-update-actions'>
      {has('install') && <button type='button' className='ob-primary' disabled={busy} onClick={() => void updater.install()}>Download &amp; install</button>}
      {has('retry') && <button type='button' className='ob-primary' disabled={busy} onClick={() => void updater.retry()}>Retry</button>}
      {has('cancel') && <button type='button' className='ob-secondary' disabled={busy} onClick={() => void updater.cancel()}>Cancel</button>}
      {(has('later') || has('dismiss')) && <button type='button' className='ob-secondary' onClick={updater.dismissBanner}>{has('later') ? 'Later' : 'Dismiss'}</button>}
    </div>}
  </>
}

/**
 * The strip under the top bar: an update is ready, downloading, installing, or
 * failed. Nothing outside the desktop app, and nothing the user put away
 * unless it is working or has something new to say.
 */
export function UpdateBanner({ updater }: { updater: DesktopUpdater }) {
  if (!updater.available) return null
  const view = bannerView(updater.status, updater.dismissed)
  if (!view) return null
  return <div className={`ob-update-banner is-${view.kind}`} role={view.kind === 'error' ? 'alert' : undefined} aria-live={view.kind === 'available' || view.kind === 'installing' ? 'polite' : undefined}>
    <Body view={view} updater={updater}/>
    {updater.actionError && view.kind !== 'error' && <p className='ob-update-problem' role='alert'>{updater.actionError}</p>}
  </div>
}

/** The sidebar's compact "Update ready" / "Downloading 42%" line. It opens Settings › App updates. */
export function UpdatePill({ updater, onOpen }: { updater: DesktopUpdater; onOpen: () => void }) {
  const text = updater.available ? pillText(updater.status) : null
  if (!text) return null
  return <button type='button' className='ob-update-pill' title='Open App updates' onClick={onOpen}>
    <span className={`ob-status-dot${updater.status?.stage === 'downloading' ? ' is-loading' : ''}`} aria-hidden='true'/>
    <strong>{text}</strong>
  </button>
}
