import { useData } from '../data/DataContext'
import { useLiveState } from '../state/liveStore'
import type { EventQuery } from '../data/source'
import type { NvrEvent } from '../types/api'
import { useAsync, type AsyncState } from './useAsync'

/** Events matching `query`; `error` is an UnavailableError while the backend has none. */
export function useEvents(query: EventQuery): AsyncState<NvrEvent[]> {
  const { source } = useData()
  // Refetch when the server announces an event change.
  const version = useLiveState(source.store, (s) => s.eventsVersion)
  return useAsync(() => source.events(query), [source, query.camera, query.fromMs, query.toMs, query.kinds?.join(','), query.zones?.join(','), version])
}

/** Event counts per hour over the last `hours` hours, oldest first. */
export function bucketByHour(events: NvrEvent[], hours: number, now: number): number[] {
  const out = new Array<number>(hours).fill(0)
  for (const e of events) {
    const age = Math.floor((now - e.start_ms) / 3_600_000)
    if (age >= 0 && age < hours) out[hours - 1 - age] = (out[hours - 1 - age] ?? 0) + 1
  }
  return out
}
