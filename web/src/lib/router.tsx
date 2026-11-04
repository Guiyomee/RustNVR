// Minimal client-side router (History API), to avoid a routing dependency.
// The Axum server answers unknown non-API paths with index.html.

import { useSyncExternalStore, type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from 'react'

const listeners = new Set<() => void>()

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  window.addEventListener('popstate', listener)
  return () => {
    listeners.delete(listener)
    window.removeEventListener('popstate', listener)
  }
}

function snapshot(): string {
  return window.location.pathname + window.location.search
}

export function navigate(to: string, opts: { replace?: boolean } = {}): void {
  if (to === snapshot()) return
  if (opts.replace) window.history.replaceState(null, '', to)
  else window.history.pushState(null, '', to)
  listeners.forEach((l) => l())
  if (!opts.replace) window.scrollTo(0, 0)
}

export interface Location {
  path: string
  query: URLSearchParams
}

export function useLocation(): Location {
  const href = useSyncExternalStore(subscribe, snapshot)
  const [path = '/', search = ''] = href.split('?')
  return { path, query: new URLSearchParams(search) }
}

/** Matches `/cameras/:id` style patterns; returns the parameters or null. */
export function matchPath(pattern: string, path: string): Record<string, string> | null {
  const p = pattern.split('/').filter(Boolean)
  const s = path.split('/').filter(Boolean)
  if (p.length !== s.length) return null
  const params: Record<string, string> = {}
  for (let i = 0; i < p.length; i++) {
    const part = p[i] ?? ''
    const seg = s[i] ?? ''
    if (part.startsWith(':')) params[part.slice(1)] = decodeURIComponent(seg)
    else if (part !== seg) return null
  }
  return params
}

type LinkProps = AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; children: ReactNode }

export function Link({ to, onClick, children, ...rest }: LinkProps) {
  const handle = (e: MouseEvent<HTMLAnchorElement>) => {
    onClick?.(e)
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
    e.preventDefault()
    navigate(to)
  }
  return (
    <a href={to} onClick={handle} {...rest}>
      {children}
    </a>
  )
}
