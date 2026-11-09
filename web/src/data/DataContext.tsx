import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { describeError } from '../api/client'
import { DEMO_SITE } from '../lib/build'
import { useLiveState, type LiveState } from '../state/liveStore'
import type { Capabilities, CapabilityKey } from '../types/api'
import { createDemoSource } from './demo/demoSource'
import { createLiveSource } from './liveSource'
import type { DataMode, DataSource } from './source'

/** The demo is a separate build (GitHub Pages); RustNVR serves the real data only. */
const MODE: DataMode = DEMO_SITE ? 'demo' : 'live'

// The demo mode of earlier versions was remembered in the browser.
try {
  window.localStorage.removeItem('rustnvr.mode')
} catch {
  // Storage unavailable: nothing to forget.
}

interface DataContextValue {
  source: DataSource
  mode: DataMode
  capabilities: Capabilities | null
  capabilitiesError: string | null
}

const Ctx = createContext<DataContextValue | null>(null)

export function DataProvider({ children }: { children: (mode: DataMode) => ReactNode }) {
  const mode = MODE
  const source = useMemo(() => (DEMO_SITE ? createDemoSource() : createLiveSource()), [])
  const [capabilities, setCapabilities] = useState<Capabilities | null>(null)
  const [capabilitiesError, setCapabilitiesError] = useState<string | null>(null)

  useEffect(() => source.start(), [source])

  useEffect(() => {
    let cancelled = false
    setCapabilities(null)
    setCapabilitiesError(null)
    const load = () =>
      source.capabilities().then(
        (c) => !cancelled && setCapabilities(c),
        (e: unknown) => {
          if (cancelled) return
          setCapabilitiesError(describeError(e))
          // Retry: the server may simply not be up yet.
          window.setTimeout(() => !cancelled && void load(), 5000)
        },
      )
    void load()
    return () => {
      cancelled = true
    }
  }, [source])

  const value = useMemo<DataContextValue>(
    () => ({
      source,
      mode,
      capabilities,
      capabilitiesError,
    }),
    [source, mode, capabilities, capabilitiesError],
  )
  return <Ctx.Provider value={value}>{children(mode)}</Ctx.Provider>
}

export function useData(): DataContextValue {
  const v = useContext(Ctx)
  if (!v) throw new Error('useData outside DataProvider')
  return v
}

export function useLive<T>(select: (s: LiveState) => T): T {
  return useLiveState(useData().source.store, select)
}

/** Whether a backend feature exists. `null` while capabilities are loading. */
export function useCapability(key: CapabilityKey): boolean | null {
  const { capabilities } = useData()
  // A feature unknown to an older server: absent.
  return capabilities ? (capabilities[key] ?? false) : null
}

export const useIsDemo = () => useData().mode === 'demo'
