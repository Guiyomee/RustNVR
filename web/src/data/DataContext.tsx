import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from 'react'
import { describeError } from '../api/client'
import { useLiveState, type LiveState } from '../state/liveStore'
import type { Capabilities, CapabilityKey } from '../types/api'
import { createDemoSource } from './demo/demoSource'
import { createLiveSource } from './liveSource'
import type { DataMode, DataSource } from './source'

const MODE_KEY = 'rustnvr.mode'

/** Demo mode is opt-in: `?demo=1` in the URL or the toggle in Settings. */
function initialMode(): DataMode {
  const q = new URLSearchParams(window.location.search).get('demo')
  if (q === '1') return 'demo'
  if (q === '0') return 'live'
  return window.localStorage.getItem(MODE_KEY) === 'demo' ? 'demo' : 'live'
}

interface DataContextValue {
  source: DataSource
  mode: DataMode
  setMode: (mode: DataMode) => void
  capabilities: Capabilities | null
  capabilitiesError: string | null
}

const Ctx = createContext<DataContextValue | null>(null)

export function DataProvider({ children }: { children: (mode: DataMode) => ReactNode }) {
  const [mode, setModeState] = useState<DataMode>(initialMode)
  const source = useMemo(() => (mode === 'demo' ? createDemoSource() : createLiveSource()), [mode])
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
      setMode: (m) => {
        window.localStorage.setItem(MODE_KEY, m)
        setModeState(m)
      },
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
