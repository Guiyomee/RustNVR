// The interface's only entry point to data. Two implementations exist and are
// never mixed: `liveSource` (the real RustNVR API) and `demo/demoSource`
// (clearly labeled fictitious data). Switching mode re-creates the source and
// remounts the application.

import type { Capabilities, NvrEvent, Recording, Storage, EventKind } from '../types/api'
import type { LiveStore } from '../state/liveStore'

export type DataMode = 'live' | 'demo'

export interface EventQuery {
  camera: string | null
  fromMs: number
  toMs: number
  kinds: EventKind[] | null
}

export interface DataSource {
  readonly mode: DataMode
  readonly store: LiveStore
  /** Starts live updates; returns a function stopping them. */
  start(): () => void
  capabilities(): Promise<Capabilities>
  recordings(camera: string, fromMs: number, toMs: number, signal?: AbortSignal): Promise<Recording[]>
  storage(): Promise<Storage>
  /** Rejects with UnavailableError while the backend has no events. */
  events(query: EventQuery): Promise<NvrEvent[]>
}
