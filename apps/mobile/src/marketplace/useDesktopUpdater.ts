import { useCallback, useEffect, useRef, useState } from 'react'
import {
  cancelUpdateDownload,
  checkForUpdates,
  getUpdateStatus,
  installAvailableUpdate,
  isDesktopUpdaterAvailable,
  listenForUpdateStatus,
  setAutomaticUpdates,
  type UpdateStatus
} from '../updater'
import { dismissKey, retryAction } from '../updateView'

/** How often to ask the shell while it is working, in case an event is missed. */
const POLL_MS = 400

const failed = (err: unknown) => (err instanceof Error && err.message ? err.message : 'The update could not be started.')

/**
 * The desktop shell's update status and the actions on it. One instance per
 * page; the banner, Settings, and the sidebar all read it. Without a shell
 * (a browser) `available` is false and nothing is loaded or polled.
 */
export function useDesktopUpdater() {
  const available = isDesktopUpdaterAvailable()
  const [status, setStatus] = useState<UpdateStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState('')
  const [dismissedKey, setDismissedKey] = useState<string | null>(null)
  const live = useRef(true)

  const receive = useCallback((next: UpdateStatus) => {
    if (!live.current) return
    setStatus(next)
    // Working again: the next offer or error is news, even if the last was put away.
    if (next.stage === 'downloading' || next.stage === 'installing') setDismissedKey(null)
  }, [])

  useEffect(() => {
    live.current = true
    if (!available) return
    let stop = () => {}
    void getUpdateStatus().then(first => first && receive(first), () => {})
    void listenForUpdateStatus(receive).then(unlisten => {
      if (live.current) stop = unlisten
      else unlisten()
    })
    return () => {
      live.current = false
      stop()
    }
  }, [available, receive])

  const working = status?.stage === 'checking' || status?.stage === 'downloading' || status?.stage === 'installing'
  useEffect(() => {
    if (!available || !working) return
    const timer = setInterval(() => void getUpdateStatus().then(next => next && receive(next), () => {}), POLL_MS)
    return () => clearInterval(timer)
  }, [available, working, receive])

  const act = useCallback(
    async (action: () => Promise<UpdateStatus | null>) => {
      setBusy(true)
      setActionError('')
      try {
        const next = await action()
        if (next) receive(next)
      } catch (err) {
        if (live.current) setActionError(failed(err))
      } finally {
        if (live.current) setBusy(false)
      }
    },
    [receive]
  )

  const check = useCallback(() => act(checkForUpdates), [act])
  const install = useCallback(() => act(installAvailableUpdate), [act])
  const cancel = useCallback(() => act(cancelUpdateDownload), [act])
  const toggleAutomatic = useCallback((enabled: boolean) => act(() => setAutomaticUpdates(enabled)), [act])
  const retry = useCallback(() => (status && retryAction(status) === 'install' ? install() : check()), [status, install, check])
  const dismissBanner = useCallback(() => setDismissedKey(status ? dismissKey(status) : null), [status])

  return {
    available,
    status,
    busy,
    actionError,
    dismissed: status !== null && dismissedKey === dismissKey(status),
    check,
    install,
    cancel,
    retry,
    toggleAutomatic,
    dismissBanner
  }
}

export type DesktopUpdater = ReturnType<typeof useDesktopUpdater>
