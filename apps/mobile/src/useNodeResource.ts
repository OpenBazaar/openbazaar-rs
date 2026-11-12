import { useCallback, useEffect, useRef, useState } from 'react'
import { afterShopPair } from './catalog'

// Resources are scoped to mounted panels. Background browser tabs pause polling;
// returning to the app refreshes immediately, without overlapping requests.
export function useNodeResource<T>(
  fetcher: () => Promise<T>,
  intervalMs: number | ((data: T | null) => number),
  enabled = true,
  stopWhen?: (data: T) => boolean,
  isEqual?: (previous: T, next: T) => boolean
) {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(enabled)
  const latestData = useRef<T | null>(null)
  const stopRef = useRef(stopWhen)
  stopRef.current = stopWhen
  const fetchRef = useRef(fetcher)
  fetchRef.current = fetcher
  const intervalRef = useRef(intervalMs)
  intervalRef.current = intervalMs
  const equalRef = useRef(isEqual)
  equalRef.current = isEqual
  const active = useRef(false)
  const generation = useRef(0)
  const inFlight = useRef<Promise<void> | null>(null)
  const queued = useRef<Promise<void> | null>(null)
  const scheduleRef = useRef<(() => void) | null>(null)
  const refresh = useCallback(
    async function refreshResource(): Promise<void> {
      if (!enabled || !active.current) return
      // The request in flight may predate this call (a payment, a new address,
      // a poll tick), so callers share one more request after it finishes.
      if (inFlight.current) {
        queued.current ??= inFlight.current.then(() => {
          queued.current = null
          return refreshResource()
        })
        return queued.current
      }
      const current = generation.current
      const pending = (async () => {
        try {
          const next = await Promise.resolve().then(() => fetchRef.current())
          if (generation.current === current) {
            if (latestData.current === null || !equalRef.current?.(latestData.current, next)) {
              latestData.current = next
              setData(next)
            }
            setError('')
          }
        } catch (e) {
          if (generation.current === current)
            setError(
              e instanceof Error ? e.message : 'The node could not be reached.'
            )
        } finally {
          inFlight.current = null
          if (generation.current === current) {
            setLoading(false)
            scheduleRef.current?.()
          }
        }
      })()
      inFlight.current = pending
      await pending
    },
    [enabled]
  )

  useEffect(() => {
    generation.current += 1
    active.current = enabled
    if (!enabled) return
    setLoading(true)
    const visible = () =>
      typeof document === 'undefined' || document.visibilityState !== 'hidden'
    let timer: ReturnType<typeof setTimeout> | undefined
    let sequence = 0
    let disposed = false
    const schedule = () => {
      clearTimeout(timer)
      if (disposed || !visible()) return
      const delay = typeof intervalRef.current === 'function' ? intervalRef.current(latestData.current) : intervalRef.current
      if (delay > 0 && !(latestData.current && stopRef.current?.(latestData.current))) timer = setTimeout(() => void poll(), delay)
    }
    scheduleRef.current = schedule
    const poll = async (force = false) => {
      const current = ++sequence
      clearTimeout(timer)
      if (!visible()) return
      if (force || !(latestData.current && stopRef.current?.(latestData.current))) await refresh()
      if (disposed || current !== sequence || !visible()) return
      schedule()
    }
    // Returning to a tab also discovers new orders in an otherwise settled list.
    const visibilityChanged = () => { void poll(true) }
    void poll(true)
    if (typeof document !== 'undefined')
      document.addEventListener('visibilitychange', visibilityChanged)
    return () => {
      disposed = true
      scheduleRef.current = null
      sequence++
      active.current = false
      generation.current += 1
      clearTimeout(timer)
      if (typeof document !== 'undefined')
        document.removeEventListener('visibilitychange', visibilityChanged)
    }
  }, [refresh, typeof intervalMs === 'number' ? intervalMs : null])
  // A request that failed for want of a pairing code can succeed once one is typed.
  useEffect(() => afterShopPair(() => void refresh()), [refresh])
  const update = useCallback((next: T) => {
    if (!active.current) return
    generation.current += 1
    latestData.current = next
    setData(next)
    setError('')
    setLoading(false)
    scheduleRef.current?.()
  }, [])
  return { data, error, loading, refresh, update }
}
