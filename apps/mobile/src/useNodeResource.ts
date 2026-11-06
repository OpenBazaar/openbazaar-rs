import { useCallback, useEffect, useRef, useState } from 'react'

// Resources are scoped to mounted panels. Background browser tabs pause polling;
// returning to the app refreshes immediately, without overlapping requests.
export function useNodeResource<T>(
  fetcher: () => Promise<T>,
  intervalMs: number,
  enabled = true,
  stopWhen?: (data: T) => boolean
) {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(enabled)
  const latestData = useRef<T | null>(null)
  const stopRef = useRef(stopWhen)
  stopRef.current = stopWhen
  const fetchRef = useRef(fetcher)
  fetchRef.current = fetcher
  const active = useRef(false)
  const generation = useRef(0)
  const inFlight = useRef<Promise<void> | null>(null)
  const completedGeneration = useRef(-1)
  const refresh = useCallback(
    async function refreshResource(): Promise<void> {
      if (!enabled || !active.current) return
      const current = generation.current
      if (inFlight.current) {
        await inFlight.current
        if (
          generation.current === current &&
          completedGeneration.current !== current
        )
          await refreshResource()
        return
      }
      const pending = (async () => {
        try {
          const next = await Promise.resolve().then(() => fetchRef.current())
          if (generation.current === current) {
            latestData.current = next
            setData(next)
            setError('')
          }
        } catch (e) {
          if (generation.current === current)
            setError(
              e instanceof Error ? e.message : 'The node could not be reached.'
            )
        } finally {
          completedGeneration.current = current
          inFlight.current = null
          if (generation.current === current) setLoading(false)
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
    void refresh()
    const visible = () =>
      typeof document === 'undefined' || document.visibilityState !== 'hidden'
    const poll = () => {
      if (
        visible() &&
        !(latestData.current && stopRef.current?.(latestData.current))
      )
        void refresh()
    }
    const timer = intervalMs > 0 ? setInterval(poll, intervalMs) : undefined
    if (typeof document !== 'undefined')
      document.addEventListener('visibilitychange', poll)
    return () => {
      active.current = false
      generation.current += 1
      if (timer) clearInterval(timer)
      if (typeof document !== 'undefined')
        document.removeEventListener('visibilitychange', poll)
    }
  }, [refresh, intervalMs])
  const update = useCallback((next: T) => {
    if (!active.current) return
    generation.current += 1
    latestData.current = next
    setData(next)
    setError('')
    setLoading(false)
  }, [])
  return { data, error, loading, refresh, update }
}
