import { useEffect, useRef, useState, type RefObject } from 'react'

/** True once the element has been in the viewport (for lazy loading), and while it is. */
export function useInView<T extends Element>(rootMargin = '200px'): [RefObject<T | null>, boolean] {
  const ref = useRef<T | null>(null)
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof IntersectionObserver === 'undefined') {
      setVisible(true)
      return
    }
    const obs = new IntersectionObserver(([entry]) => setVisible(entry?.isIntersecting ?? false), { rootMargin })
    obs.observe(el)
    return () => obs.disconnect()
  }, [rootMargin])
  return [ref, visible]
}

/** False while the browser tab is hidden. */
export function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState === 'visible')
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === 'visible')
    document.addEventListener('visibilitychange', on)
    return () => document.removeEventListener('visibilitychange', on)
  }, [])
  return visible
}

/** Current time, refreshed every `ms`. */
export function useNow(ms = 30_000): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), ms)
    return () => window.clearInterval(id)
  }, [ms])
  return now
}

/** Boolean persisted in localStorage. */
export function useStoredFlag(key: string, initial: boolean): [boolean, (v: boolean) => void] {
  const [v, setV] = useState(() => {
    const s = window.localStorage.getItem(key)
    return s === null ? initial : s === '1'
  })
  return [
    v,
    (next) => {
      window.localStorage.setItem(key, next ? '1' : '0')
      setV(next)
    },
  ]
}
