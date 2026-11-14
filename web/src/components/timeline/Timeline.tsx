import { memo, useMemo, useState, type MouseEvent } from 'react'
import { withBase } from '../../lib/build'
import { formatTime } from '../../lib/format'
import type { Period } from './periods'
import { t } from '../../lib/i18n'

export interface TimelineRow {
  id: string
  label: string
}

export interface TimelineMarker {
  id: string
  at: number
  label: string
  /** Objects were recognized (the label names them): marked apart. */
  objects?: boolean
  /** Picture shown when the pointer is over the marker. */
  thumbnail?: string | null
}

export interface TimelineProps {
  rows: TimelineRow[]
  start: number
  end: number
  recordings: Record<string, Period[]>
  /** `null`: the backend has no motion data (legend says "non disponible"). */
  motion: Record<string, Period[]> | null
  /** `null`: the backend has no events. */
  events: Record<string, TimelineMarker[]> | null
  /** Bookmarks by row (playback); not given: none shown. */
  bookmarks?: Record<string, TimelineMarker[]>
  now: number
  /** Playback position. */
  cursor?: number | null
  onSelect?: (rowId: string, t: number) => void
  /** Highlight one row (playback). */
  activeRow?: string | null
}

const HOUR = 3_600_000

function tickStep(spanMs: number): number {
  if (spanMs <= 1.5 * HOUR) return 10 * 60_000
  if (spanMs <= 6.5 * HOUR) return HOUR
  if (spanMs <= 12.5 * HOUR) return 2 * HOUR
  return 3 * HOUR
}

function covers(periods: Period[] | undefined, t: number): boolean {
  return periods?.some((p) => p.start <= t && t < p.end) ?? false
}

/**
 * Horizontal timeline, one row per camera. Pure presentation: data and
 * navigation state come from the parent (TimelineCard, Playback page).
 */
export const Timeline = memo(function Timeline({ rows, start, end, recordings, motion, events, bookmarks, now, cursor, onSelect, activeRow }: TimelineProps) {
  const span = end - start
  const pct = (t: number) => ((t - start) / span) * 100
  const [hover, setHover] = useState<{ row: string; t: number; x: number } | null>(null)
  const [peek, setPeek] = useState<{ marker: TimelineMarker; x: number; y: number } | null>(null)

  const ticks = useMemo(() => {
    const step = tickStep(span)
    const offset = new Date(start).getTimezoneOffset() * 60_000
    const first = Math.ceil((start - offset) / step) * step + offset
    const out: number[] = []
    for (let t = first; t <= end; t += step) out.push(t)
    return out
  }, [start, end, span])

  const pointer = (rowId: string) => (e: MouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect()
    const x = Math.max(0, Math.min(r.width, e.clientX - r.left))
    setHover({ row: rowId, t: start + (x / r.width) * span, x: (x / r.width) * 100 })
  }

  const clip = (p: Period) => {
    const s = Math.max(p.start, start)
    const e = Math.min(p.end, end)
    return e > s ? { left: `${pct(s)}%`, width: `max(1px, ${pct(e) - pct(s)}%)` } : null
  }

  return (
    <div className="select-none">
      <div className="flex">
        <div className="w-24 shrink-0 sm:w-32" />
        <div className="relative h-6 flex-1 text-[11px] text-muted">
          {ticks.map((t) => (
            <span key={t} className="tabular absolute top-1 -translate-x-1/2" style={{ left: `${pct(t)}%` }}>
              {formatTime(t)}
            </span>
          ))}
        </div>
      </div>

      <div className="space-y-1.5">
        {rows.map((row) => {
          const rec = recordings[row.id]
          return (
            <div key={row.id} className="flex items-center">
              <div className={`w-24 shrink-0 truncate pr-3 text-xs sm:w-32 ${activeRow === row.id ? 'font-semibold text-fg' : 'text-muted'}`} title={row.label}>
                {row.label}
              </div>
              <div
                className={`relative h-7 flex-1 overflow-hidden rounded bg-bg-2 ${onSelect ? 'cursor-pointer' : ''} ${activeRow === row.id ? 'ring-1 ring-accent/60' : ''}`}
                onMouseMove={pointer(row.id)}
                onMouseLeave={() => setHover(null)}
                onClick={(e) => {
                  if (!onSelect) return
                  const r = e.currentTarget.getBoundingClientRect()
                  onSelect(row.id, start + ((e.clientX - r.left) / r.width) * span)
                }}
              >
                {ticks.map((t) => (
                  <div key={t} className="absolute inset-y-0 w-px bg-line/60" style={{ left: `${pct(t)}%` }} aria-hidden />
                ))}
                {rec?.map((p) => {
                  const style = clip(p)
                  return style && <div key={`r${p.start}`} className="absolute inset-y-1.5 rounded-sm bg-accent" style={style} />
                })}
                {motion?.[row.id]?.map((p) => {
                  const style = clip(p)
                  return style && <div key={`m${p.start}`} className="absolute bottom-1.5 h-1.5 rounded-sm bg-motion ring-2 ring-bg-2" style={style} />
                })}
                {events?.[row.id]?.map(
                  (m) =>
                    m.at >= start &&
                    m.at <= end && (
                      <div
                        key={m.id}
                        title={m.thumbnail ? undefined : `${m.label} — ${formatTime(m.at, true)}`}
                        className={`absolute top-0.5 -translate-x-1/2 rotate-45 ring-2 ring-bg-2 ${m.objects ? 'size-2.5 bg-ok' : 'size-2 bg-fg'}`}
                        style={{ left: `${pct(m.at)}%` }}
                        onMouseEnter={(e) => {
                          const r = e.currentTarget.getBoundingClientRect()
                          setPeek({ marker: m, x: r.left + r.width / 2, y: r.top })
                        }}
                        onMouseLeave={() => setPeek(null)}
                      />
                    ),
                )}
                {bookmarks?.[row.id]?.map(
                  (m) =>
                    m.at >= start &&
                    m.at <= end && (
                      <div
                        key={`b${m.id}`}
                        title={`${m.label} — ${formatTime(m.at, true)}`}
                        className="absolute inset-y-0 w-0.5 -translate-x-1/2 bg-warn"
                        style={{ left: `${pct(m.at)}%` }}
                      >
                        <span className="absolute -top-px left-1/2 size-2.5 -translate-x-1/2 rounded-b-sm bg-warn" aria-hidden />
                      </div>
                    ),
                )}
                {now >= start && now <= end && (
                  <div className="absolute inset-y-0 border-l-2 border-dashed border-muted" style={{ left: `${pct(now)}%` }} aria-hidden />
                )}
                {cursor != null && cursor >= start && cursor <= end && (
                  <div className="absolute inset-y-0 w-0.5 bg-fg shadow-[0_0_0_1px_var(--color-bg-2)]" style={{ left: `${pct(cursor)}%` }} aria-hidden />
                )}
                {hover?.row === row.id && (
                  <>
                    <div className="pointer-events-none absolute inset-y-0 w-px bg-fg/50" style={{ left: `${hover.x}%` }} />
                    <div
                      className={`pointer-events-none absolute top-1/2 z-10 -translate-y-1/2 rounded border border-line bg-bg px-1.5 py-0.5 text-[11px] whitespace-nowrap shadow ${
                        hover.x > 70 ? '-translate-x-[calc(100%+6px)]' : 'translate-x-1.5'
                      }`}
                      style={{ left: `${hover.x}%` }}
                    >
                      <span className="tabular">{formatTime(hover.t, true)}</span>
                      <span className="text-muted">
                        {' · '}
                        {hover.t > now ? 'futur' : covers(rec, hover.t) ? t('enregistré', 'recorded') : t('non enregistré', 'not recorded')}
                        {covers(motion?.[row.id], hover.t) ? t(' · mouvement', ' · motion') : ''}
                      </span>
                    </div>
                  </>
                )}
              </div>
            </div>
          )
        })}
      </div>

      {peek?.marker.thumbnail && (
        <div
          className="pointer-events-none fixed z-50 w-40 -translate-x-1/2 -translate-y-full overflow-hidden rounded-md border border-line bg-bg shadow-lg"
          style={{ left: Math.min(Math.max(peek.x, 84), window.innerWidth - 84), top: peek.y - 6 }}
        >
          <img src={withBase(peek.marker.thumbnail)} alt="" className="aspect-video w-full object-cover" />
          <p className="truncate px-1.5 py-0.5 text-[11px]">
            <span className="tabular">{formatTime(peek.marker.at, true)}</span> · {peek.marker.label}
          </p>
        </div>
      )}
      <TimelineLegend motionAvailable={motion !== null} eventsAvailable={events !== null} showCursor={cursor != null} showBookmarks={bookmarks !== undefined} />
    </div>
  )
})

function TimelineLegend({ motionAvailable, eventsAvailable, showCursor, showBookmarks }: { motionAvailable: boolean; eventsAvailable: boolean; showCursor: boolean; showBookmarks: boolean }) {
  return (
    <ul className="mt-3 flex flex-wrap gap-x-4 gap-y-1 pl-24 text-xs text-muted sm:pl-32">
      <li className="flex items-center gap-1.5">
        <span className="h-2.5 w-4 rounded-sm bg-accent" aria-hidden />{t(' Enregistré', ' Recorded')}
      </li>
      <li className={`flex items-center gap-1.5 ${motionAvailable ? '' : 'opacity-60'}`}>
        <span className="h-1.5 w-4 rounded-sm bg-motion" aria-hidden />{t(' Mouvement', ' Motion')}{!motionAvailable && t(' (non disponible)', ' (not available)')}
      </li>
      <li className={`flex items-center gap-1.5 ${eventsAvailable ? '' : 'opacity-60'}`}>
        <span className="size-2 rotate-45 bg-fg" aria-hidden />{t(' Événement', ' Event')}{!eventsAvailable && t(' (non disponible)', ' (not available)')}
      </li>
      {eventsAvailable && (
        <li className="flex items-center gap-1.5">
          <span className="size-2 rotate-45 bg-ok" aria-hidden />{t(' Objet reconnu', ' Recognized object')}
        </li>
      )}
      <li className="flex items-center gap-1.5">
        <span className="h-3 border-l-2 border-dashed border-muted" aria-hidden />{t(' Maintenant', ' Now')}
      </li>
      {showBookmarks && (
        <li className="flex items-center gap-1.5">
          <span className="h-3 w-0.5 bg-warn" aria-hidden />{t(' Signet', ' Bookmark')}
        </li>
      )}
      {showCursor && (
        <li className="flex items-center gap-1.5">
          <span className="h-3 w-0.5 bg-fg" aria-hidden />{t(' Position de lecture', ' Playback position')}
        </li>
      )}
    </ul>
  )
}
