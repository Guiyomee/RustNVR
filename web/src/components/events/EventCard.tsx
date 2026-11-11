import { Activity } from 'lucide-react'
import { memo } from 'react'
import { useIsDemo } from '../../data/DataContext'
import { demoPicture, demoSpec } from '../../data/demo/fixtures'
import { useInView } from '../../hooks/useUi'
import { withBase } from '../../lib/build'
import { formatDate, formatDuration, formatTime } from '../../lib/format'
import { objectLabel } from '../../lib/objects'
import type { DetectedObject, EventKind, NvrEvent } from '../../types/api'
import { Badge } from '../ui/Card'
import { t } from '../../lib/i18n'

export const EVENT_KIND_LABEL: Record<EventKind, string> = {
  motion: t('Mouvement', 'Motion'),
  manual: t('Manuel', 'Manual'),
  person: t('Personne', 'Person'),
  vehicle: t('Véhicule', 'Vehicle'),
  animal: t('Animal', 'Animal'),
}

/** French names of common labels (COCO classes used by DeepStack / CodeProject.AI). */
/** Distinct recognized labels, most confident first (e.g. « Personne 87 % »). */
function objectsText(objects: DetectedObject[]): string {
  const best = new Map<string, number>()
  for (const o of objects) best.set(o.label, Math.max(best.get(o.label) ?? 0, o.score))
  const zones = new Map(objects.filter((o) => o.zone).map((o) => [o.label, o.zone]))
  return [...best].map(([l, s]) => `${objectLabel(l)} ${Math.round(s * 100)} %${zones.get(l) ? ` (${zones.get(l)})` : ''}`).join(', ')
}

/** Thumbnail, loaded only once visible. Demo events get a drawn placeholder. */
function Thumbnail({ event, className }: { event: NvrEvent; className: string }) {
  const demo = useIsDemo()
  const [ref, visible] = useInView<HTMLDivElement>()
  const hue = demoSpec(event.camera)?.hue ?? 210
  return (
    <div ref={ref} className={`relative overflow-hidden bg-bg-2 ${className}`}>
      {visible && event.thumbnail_url ? (
        <>
          <img src={withBase(event.thumbnail_url)} alt="" loading="lazy" decoding="async" className="h-full w-full object-cover" />
          {event.objects.map((o, i) =>
            o.bbox ? (
              <div
                key={i}
                className="pointer-events-none absolute rounded-sm border-2 border-accent"
                style={{ left: `${o.bbox[0] * 100}%`, top: `${o.bbox[1] * 100}%`, width: `${(o.bbox[2] - o.bbox[0]) * 100}%`, height: `${(o.bbox[3] - o.bbox[1]) * 100}%` }}
                aria-hidden
              />
            ) : null,
          )}
        </>
      ) : demo ? (
        <div className="h-full w-full" style={{ background: `linear-gradient(160deg, hsl(${hue} 30% 28%), hsl(${hue} 20% 12%))` }} aria-hidden>
          {visible && demoPicture(event.camera) && <img src={demoPicture(event.camera) ?? ''} alt="" loading="lazy" decoding="async" className="h-full w-full object-cover" />}
          <span className="absolute right-1 bottom-1 rounded bg-black/55 px-1 text-[9px] font-semibold text-warn">{t('DÉMO', 'DEMO')}</span>
        </div>
      ) : (
        <div className="flex h-full w-full items-center justify-center text-muted" aria-hidden>
          <Activity className="size-4" />
        </div>
      )}
    </div>
  )
}

/** Compact row for the overview column and the list view. */
export const EventRow = memo(function EventRow({ event, cameraName, onClick }: { event: NvrEvent; cameraName: string; onClick?: () => void }) {
  return (
    <li>
      <button type="button" onClick={onClick} className="flex w-full items-center gap-3 px-4 py-2.5 text-left hover:bg-line/40">
        <Thumbnail event={event} className="h-10 w-16 shrink-0 rounded" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="size-2 shrink-0 rounded-full bg-motion" aria-hidden />
            <span className="truncate text-sm font-medium">{EVENT_KIND_LABEL[event.kind]}</span>
          </div>
          <p className="truncate text-xs text-muted">
            {cameraName}
            {event.objects.length > 0 && ` · ${objectsText(event.objects)}`}
          </p>
        </div>
        <div className="shrink-0 text-right">
          <div className="tabular text-xs">{formatTime(event.start_ms)}</div>
          <div className="tabular text-[11px] text-muted">{event.end_ms === null ? t('en cours', 'ongoing') : formatDuration(event.end_ms - event.start_ms)}</div>
        </div>
      </button>
    </li>
  )
})

/** Grid tile for the events gallery. */
export const EventCard = memo(function EventCard({ event, cameraName, onClick }: { event: NvrEvent; cameraName: string; onClick?: () => void }) {
  const demo = useIsDemo()
  return (
    <button type="button" onClick={onClick} className="card overflow-hidden text-left transition-colors hover:border-accent/60">
      <Thumbnail event={event} className="aspect-video w-full" />
      <div className="space-y-1 p-3">
        <div className="flex items-center justify-between gap-2">
          <span className="flex items-center gap-1.5 text-sm font-medium">
            <span className="size-2 rounded-full bg-motion" aria-hidden />
            {EVENT_KIND_LABEL[event.kind]}
          </span>
          {demo ? <Badge tone="demo">{t('Démo', 'Demo')}</Badge> : event.end_ms === null && <Badge tone="warn">{t('En cours', 'Ongoing')}</Badge>}
        </div>
        <p className="truncate text-xs text-muted">
          {cameraName}
          {event.zones.length > 0 && ` · ${event.zones.join(', ')}`}
        </p>
        {event.objects.length > 0 && <p className="truncate text-xs font-medium text-accent">{objectsText(event.objects)}</p>}
        <p className="tabular text-xs text-muted">
          {formatDate(event.start_ms)} · {formatTime(event.start_ms, true)}
          {event.end_ms !== null && ` · ${formatDuration(event.end_ms - event.start_ms)}`}
        </p>
      </div>
    </button>
  )
})
