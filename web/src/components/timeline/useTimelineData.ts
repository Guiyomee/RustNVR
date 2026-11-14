import { UnavailableError } from '../../api/client'
import { useData } from '../../data/DataContext'
import { useAsync, type AsyncState } from '../../hooks/useAsync'
import type { NvrEvent, Recording } from '../../types/api'
import { EVENT_KIND_LABEL } from '../events/EventCard'
import { objectLabel } from '../../lib/objects'
import { eventPeriods, mergeRecordings, type Period } from './periods'
import type { TimelineMarker } from './Timeline'

export interface TimelineData {
  recordings: Record<string, Recording[]>
  periods: Record<string, Period[]>
  /** null when the backend has no events. */
  motion: Record<string, Period[]> | null
  markers: Record<string, TimelineMarker[]> | null
}

function groupBy<T>(items: T[], key: (t: T) => string): Record<string, T[]> {
  const out: Record<string, T[]> = {}
  for (const i of items) (out[key(i)] ??= []).push(i)
  return out
}

/** The timeline marker of an event: named after its recognized objects, with its picture. */
function marker(e: NvrEvent): TimelineMarker {
  const labels = [...new Set(e.objects.map((o) => objectLabel(o.label)))]
  return { id: e.id, at: e.start_ms, label: labels.length > 0 ? labels.join(', ') : EVENT_KIND_LABEL[e.kind], objects: labels.length > 0, thumbnail: e.thumbnail_url }
}

/** Recordings (real or demo, depending on the source) and events of several cameras. */
export function useTimelineData(cameraIds: string[], start: number, end: number): AsyncState<TimelineData> {
  const { source } = useData()
  const key = cameraIds.join(',')
  return useAsync(
    async (signal) => {
      const lists = await Promise.all(cameraIds.map((id) => source.recordings(id, start, end, signal)))
      const recordings: Record<string, Recording[]> = {}
      const periods: Record<string, Period[]> = {}
      cameraIds.forEach((id, i) => {
        const list = lists[i] ?? []
        recordings[id] = list
        periods[id] = mergeRecordings(list)
      })
      let events: NvrEvent[] | null = null
      try {
        events = await source.events({ camera: null, fromMs: start, toMs: end, kinds: null })
      } catch (e) {
        if (!(e instanceof UnavailableError)) throw e
      }
      const byCamera = events ? groupBy(events, (e) => e.camera) : null
      return {
        recordings,
        periods,
        motion: byCamera ? Object.fromEntries(Object.entries(byCamera).map(([k, v]) => [k, eventPeriods(v)])) : null,
        markers: byCamera
          ? Object.fromEntries(
              Object.entries(byCamera).map(([k, v]) => [k, v.map((e) => marker(e))]),
            )
          : null,
      }
    },
    [source, key, start, end],
  )
}
