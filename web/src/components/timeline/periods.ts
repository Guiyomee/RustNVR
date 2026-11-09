import type { NvrEvent, Recording } from '../../types/api'

export interface Period {
  start: number
  end: number
}

/** Merges recordings into continuous periods (gaps shorter than `toleranceMs` are bridged). */
export function mergeRecordings(recordings: Recording[], toleranceMs = 2_000): Period[] {
  const sorted = [...recordings].sort((a, b) => a.start_ms - b.start_ms)
  const out: Period[] = []
  for (const r of sorted) {
    const last = out[out.length - 1]
    if (last && r.start_ms <= last.end + toleranceMs) last.end = Math.max(last.end, r.end_ms)
    else out.push({ start: r.start_ms, end: r.end_ms })
  }
  return out
}

export function eventPeriods(events: NvrEvent[]): Period[] {
  return events.filter((e) => e.end_ms !== null).map((e) => ({ start: e.start_ms, end: e.end_ms ?? e.start_ms }))
}

/** The recording containing `t`, if any. */
export function recordingAt(recordings: Recording[], t: number): Recording | undefined {
  return recordings.find((r) => r.start_ms <= t && t < r.end_ms)
}
