import { Activity, Download } from 'lucide-react'
import { memo, useState } from 'react'
import { describeError } from '../../api/client'
import { exportJourney } from '../../api/endpoints'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { demoPicture, demoSpec } from '../../data/demo/fixtures'
import { useInView } from '../../hooks/useUi'
import { withBase } from '../../lib/build'
import { formatDate, formatDuration, formatTime } from '../../lib/format'
import { objectLabel } from '../../lib/objects'
import type { DetectedObject, EventKind, NvrEvent } from '../../types/api'
import { Badge } from '../ui/Card'
import { Button } from '../ui/Button'
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
/** A plate as written on it: `AB123CD` → `AB-123-CD` (French plates since
 * 2009); others as read. */
export function formatPlate(plate: string): string {
  const m = /^([A-Z]{2})(\d{3})([A-Z]{2})$/.exec(plate)
  return m ? `${m[1]}-${m[2]}-${m[3]}` : plate
}

function objectsText(objects: DetectedObject[]): string {
  const best = new Map<string, number>()
  for (const o of objects) best.set(o.label, Math.max(best.get(o.label) ?? 0, o.score))
  const zones = new Map(objects.filter((o) => o.zone).map((o) => [o.label, o.zone]))
  const text = [...best].map(([l, s]) => `${objectLabel(l)} ${Math.round(s * 100)} %${zones.get(l) ? ` (${zones.get(l)})` : ''}`).join(', ')
  const plates = objects.flatMap((o) => (o.plate ? [o.plate_owner ? `${formatPlate(o.plate)} (${o.plate_owner})` : formatPlate(o.plate)] : []))
  return plates.length > 0 ? `${text} · ${plates.join(', ')}` : text
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

/** An event of the list, or the events of one journey between cameras. */
export type EventItem = { kind: 'event'; event: NvrEvent } | { kind: 'journey'; id: string; events: NvrEvent[] }

/**
 * Groups the events of a journey (two of them or more in `list`, newest
 * first) at the place of its newest one; its events go oldest first.
 */
export function groupJourneys(list: NvrEvent[]): EventItem[] {
  const byJourney = new Map<string, NvrEvent[]>()
  for (const e of list) if (e.journey) byJourney.set(e.journey, [...(byJourney.get(e.journey) ?? []), e])
  const out: EventItem[] = []
  const done = new Set<string>()
  for (const e of list) {
    const group = e.journey ? byJourney.get(e.journey) : undefined
    if (!e.journey || !group || group.length < 2) {
      out.push({ kind: 'event', event: e })
    } else if (!done.has(e.journey)) {
      done.add(e.journey)
      out.push({ kind: 'journey', id: e.journey, events: [...group].sort((a, b) => a.start_ms - b.start_ms) })
    }
  }
  return out
}

/** Where a journey is played: its cameras side by side, 2 s before it starts. */
export function journeyPlaybackUrl(events: NvrEvent[]): string {
  const cameras = [...new Set(events.map((e) => e.camera))]
  const start = Math.min(...events.map((e) => e.start_ms)) - 2_000
  return `/playback?camera=${cameras.map(encodeURIComponent).join(',')}&t=${start}`
}

function journeyText(events: NvrEvent[], name: (id: string) => string) {
  const first = events[0]
  const last = events[events.length - 1]
  const labels = first?.objects.map((o) => o.label) ?? []
  const label = labels.find((l) => events.every((e) => e.objects.some((o) => o.label === l))) ?? labels[0]
  const ongoing = events.some((e) => e.end_ms === null)
  const end = Math.max(...events.map((e) => e.end_ms ?? e.start_ms))
  return {
    title: label ? `${t('Trajet', 'Journey')} · ${objectLabel(label)}` : t('Trajet', 'Journey'),
    path: events.map((e) => name(e.camera)).join(' → '),
    start: first?.start_ms ?? 0,
    duration: ongoing || !first || !last ? null : end - first.start_ms,
  }
}

/** « Export »: one MP4 of the journey, its cameras one after the other. */
function JourneyExport({ id, ongoing, compact = false }: { id: string; ongoing: boolean; compact?: boolean }) {
  const demo = useIsDemo()
  const canExport = useCapability('export') === true
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      const r = await exportJourney(id)
      const a = document.createElement('a')
      a.href = withBase(r.url)
      a.download = r.name
      document.body.append(a)
      a.click()
      a.remove()
    } catch (err) {
      source.store.notify('error', t(`Export du trajet : ${describeError(err)}`, `Journey export: ${describeError(err)}`))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Button
      size="sm"
      variant={compact ? 'ghost' : 'secondary'}
      icon={<Download className="size-3.5" />}
      aria-label={t('Exporter le trajet en un seul MP4', 'Export the journey as one MP4')}
      disabledReason={
        demo
          ? t('Indisponible en mode démo.', 'Not available in demo mode.')
          : !canExport
            ? t("L'export nécessite FFmpeg, qui n'a pas pu être lancé sur le serveur.", 'Exporting needs FFmpeg, which could not be started on the server.')
            : ongoing
              ? t('Trajet en cours : exportable à sa fin.', 'Journey ongoing: can be exported once over.')
              : busy
                ? t('Préparation du MP4…', 'Preparing the MP4…')
                : null
      }
      onClick={() => void run()}
    >
      {compact ? null : busy ? t('Préparation…', 'Preparing…') : t('Exporter', 'Export')}
    </Button>
  )
}

/** Grid tile of a journey: its pictures one after the other. */
export const JourneyCard = memo(function JourneyCard({ id, events, name, onClick }: { id: string; events: NvrEvent[]; name: (id: string) => string; onClick?: () => void }) {
  const j = journeyText(events, name)
  const shown = events.slice(0, 4)
  return (
    <div className="card overflow-hidden transition-colors hover:border-accent/60 sm:col-span-2">
      <button type="button" onClick={onClick} className="block w-full text-left" aria-label={t(`Lire le trajet ${j.path}`, `Play the journey ${j.path}`)}>
        <div className="grid aspect-[32/9] w-full gap-px bg-line" style={{ gridTemplateColumns: `repeat(${shown.length}, minmax(0, 1fr))` }}>
          {shown.map((e) => (
            <Thumbnail key={e.id} event={e} className="h-full w-full" />
          ))}
        </div>
      </button>
      <div className="flex items-start gap-2 p-3">
        <button type="button" onClick={onClick} className="min-w-0 flex-1 space-y-1 text-left">
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-1.5 text-sm font-medium">
              <span className="size-2 rounded-full bg-accent" aria-hidden />
              {j.title}
            </span>
            {j.duration === null && <Badge tone="warn">{t('En cours', 'Ongoing')}</Badge>}
          </div>
          <p className="truncate text-xs font-medium text-accent">{j.path}</p>
          <p className="tabular text-xs text-muted">
            {formatDate(j.start)} · {formatTime(j.start, true)}
            {j.duration !== null && ` · ${formatDuration(j.duration)}`}
          </p>
        </button>
        <JourneyExport id={id} ongoing={j.duration === null} />
      </div>
    </div>
  )
})

/** Compact row of a journey for the list view. */
export const JourneyRow = memo(function JourneyRow({ id, events, name, onClick }: { id: string; events: NvrEvent[]; name: (id: string) => string; onClick?: () => void }) {
  const j = journeyText(events, name)
  return (
    <li className="flex items-center gap-1 pr-2 hover:bg-line/40">
      <button type="button" onClick={onClick} className="flex min-w-0 flex-1 items-center gap-3 px-4 py-2.5 text-left">
        <div className="flex h-10 shrink-0 gap-px overflow-hidden rounded">
          {events.slice(0, 3).map((e) => (
            <Thumbnail key={e.id} event={e} className="h-10 w-12" />
          ))}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="size-2 shrink-0 rounded-full bg-accent" aria-hidden />
            <span className="truncate text-sm font-medium">{j.title}</span>
          </div>
          <p className="truncate text-xs text-muted">{j.path}</p>
        </div>
        <div className="shrink-0 text-right">
          <div className="tabular text-xs">{formatTime(j.start)}</div>
          <div className="tabular text-[11px] text-muted">{j.duration === null ? t('en cours', 'ongoing') : formatDuration(j.duration)}</div>
        </div>
      </button>
      <JourneyExport id={id} ongoing={j.duration === null} compact />
    </li>
  )
})
