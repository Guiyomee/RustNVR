import { useCallback, useEffect, useRef, useState, type DependencyList } from 'react'

export interface AsyncState<T> {
  data: T | null
  error: unknown
  loading: boolean
  reload: () => void
}

/**
 * Runs `fn` when `deps` change; the previous call is aborted. Keeps the last
 * data while reloading so that views do not flash.
 */
export function useAsync<T>(fn: (signal: AbortSignal) => Promise<T>, deps: DependencyList): AsyncState<T> {
  const [data, setData] = useState<T | null>(null)
  const [error, setError] = useState<unknown>(null)
  const [loading, setLoading] = useState(true)
  const [nonce, setNonce] = useState(0)
  const fnRef = useRef(fn)
  fnRef.current = fn

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    fnRef.current(controller.signal).then(
      (d) => {
        if (controller.signal.aborted) return
        setData(d)
        setLoading(false)
      },
      (e: unknown) => {
        if (controller.signal.aborted) return
        setError(e)
        setLoading(false)
      },
    )
    return () => controller.abort()
  }, [...deps, nonce])

  const reload = useCallback(() => setNonce((n) => n + 1), [])
  return { data, error, loading, reload }
}

/** Re-runs a loader every `ms` (paused while the tab is hidden). */
export function useInterval(callback: () => void, ms: number | null): void {
  const ref = useRef(callback)
  ref.current = callback
  useEffect(() => {
    if (ms === null) return
    const id = window.setInterval(() => {
      if (document.visibilityState === 'visible') ref.current()
    }, ms)
    return () => window.clearInterval(id)
  }, [ms])
}
