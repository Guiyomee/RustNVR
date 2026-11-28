import { Bookmark as BookmarkIcon, ChevronLeft, ChevronRight, Download, Pause, Play, Shield } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { BookmarkDialog, BookmarkList } from '../components/playback/Bookmarks'
import { ExportDialog } from '../components/playback/ExportDialog'
import { ProtectDialog, ProtectedList } from '../components/playback/ProtectDialog'
import { RecordingPlayer } from '../components/playback/RecordingPlayer'
import { EVENT_KIND_LABEL } from '../components/events/EventCard'
import { Timeline } from '../components/timeline/Timeline'
import { useTimelineData } from '../components/timeline/useTimelineData'
import { Button } from '../components/ui/Button'
import { Badge, Card } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { Segmented } from '../components/ui/Tabs'
import { useCapability, useIsDemo, useLive } from '../data/DataContext'
import { getBookmarks, getProtected } from '../api/endpoints'
import { useReadOnly } from '../auth/AuthGate'
import { useAsync, useInterval } from '../hooks/useAsync'
import { useNow } from '../hooks/useUi'
import { formatTime, fromDateInput, startOfDay, toDateInput } from '../lib/format'
import { objectLabel } from '../lib/objects'
import { navigate, useLocation } from '../lib/router'
import type { Bookmark, NvrEvent, Recording } from '../types/api'
import { t } from '../lib/i18n'

const DAY = 86_400_000
const MINUTE = 60_000
const MAX_CAMERAS = 4
const SPEEDS = [1, 2, 4, 8] as const
type Speed = (typeof SPEEDS)[number]
/** Timeline scale, in minutes. */
const ZOOMS = [
  { value: 1440, label: '24 h' },
  { value: 360, label: '6 h' },
  { value: 60, label: '1 h' },
  { value: 10, label: '10 min' },
]
/** A click on an event starts this long before it. */
const LEAD_MS = 5_000

function sortedAll(recs: Record<string, Recording[]>, ids: string[]): Recording[] {
  return ids.flatMap((id) => recs[id] ?? []).sort((a, b) => a.start_ms - b.start_ms)
}

/** « Personne 87 % », « Véhicule 92 % (AB123CD) », else the kind of event. */
function eventTitle(e: NvrEvent): string {
  const best = [...e.objects].sort((a, b) => b.score - a.score)[0]
  if (!best) return EVENT_KIND_LABEL[e.kind]
  const plate = e.objects.find((o) => o.plate)?.plate
  return `${objectLabel(best.label)} ${Math.round(best.score * 100)} %${plate ? ` (${plate})` : ''}`
}

const chip = (on: boolean) => `h-8 shrink-0 rounded-full border px-3 text-xs font-medium transition-colors disabled:opacity-40 ${on ? 'border-accent bg-accent/15 text-accent' : 'border-line text-muted hover:text-fg'}`

export function PlaybackPage() {
  const { query } = useLocation()
  const cameras = useLive((s) => s.cameras)
  const loaded = useLive((s) => s.loaded)
  const demo = useIsDemo()
  const now = useNow(30_000)

  const initialT = Number(query.get('t')) || null
  const [selected, setSelected] = useState<string[]>(() => {
    // Several cameras (a journey): `camera=a,b,c`.
    const c = query.get('camera')
    return c ? [...new Set(c.split(',').filter(Boolean))] : []
  })
  const [day, setDay] = useState(() => startOfDay(initialT ?? Date.now()))
  const [position, setPosition] = useState<number | null>(initialT)
  const [playing, setPlaying] = useState(Boolean(initialT))
  const [rate, setRate] = useState<Speed>(1)
  const [zoom, setZoom] = useState(60)
  const [exporting, setExporting] = useState(false)
  const canExport = useCapability('export') === true
  const canProtect = useCapability('protect') === true && !demo
  const readOnly = useReadOnly()
  const [protecting, setProtecting] = useState(false)
  const protectedList = useAsync(() => (canProtect ? getProtected() : Promise.resolve([])), [canProtect])
  const canBookmark = useCapability('bookmarks') === true && !demo
  const [bookmarking, setBookmarking] = useState(false)

  // Default selection once cameras are known.
  useEffect(() => {
    if (selected.length === 0 && cameras.length > 0) {
      const first = cameras.find((c) => c.enabled && c.recording_enabled) ?? cameras[0]
      if (first) setSelected([first.id])
    }
  }, [cameras, selected.length])

  const dayEnd = day + DAY
  const data = useTimelineData(selected, day, Math.min(dayEnd, Math.max(now, day + 1)))
  const isToday = day === startOfDay(now)
  useInterval(() => isToday && data.reload(), 60_000)
  const bookmarkList = useAsync((signal) => (canBookmark ? getBookmarks(day, dayEnd, signal) : Promise.resolve([] as Bookmark[])), [canBookmark, day, dayEnd])
  const shownBookmarks = useMemo(() => (bookmarkList.data ?? []).filter((b) => selected.includes(b.camera)), [bookmarkList.data, selected])
  const bookmarkMarkers = useMemo(() => {
    const out: Record<string, { id: string; at: number; label: string }[]> = {}
    for (const b of shownBookmarks) (out[b.camera] ??= []).push({ id: String(b.id), at: b.at_ms, label: b.note })
    return out
  }, [shownBookmarks])

  const recordings = data.data?.recordings ?? {}
  const all = useMemo(() => sortedAll(data.data?.recordings ?? {}, selected), [data.data, selected])
  const master = selected[0] ?? null

  // Without an explicit time, start on the latest recording of the day.
  useEffect(() => {
    if (position === null && all.length > 0) {
      const last = all[all.length - 1]
      if (last) setPosition(Math.max(last.start_ms, last.end_ms - 60_000))
    }
  }, [all, position])

  // Keep the URL shareable without adding history entries.
  useEffect(() => {
    const params = new URLSearchParams()
    if (master) params.set('camera', master)
    if (position !== null) params.set('t', String(Math.round(position)))
    navigate(`/playback?${params.toString()}`, { replace: true })
  }, [master, position === null ? null : Math.round(position / 5000)])

  const seek = useCallback((t: number) => setPosition(Math.max(day, Math.min(dayEnd - 1, t))), [day, dayEnd])
  const onGap = useCallback(
    (next: number | null) => {
      if (next === null || next >= dayEnd) setPlaying(false)
      else setPosition(next)
    },
    [dayEnd],
  )

  const toggleCamera = (id: string) =>
    setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : s.length >= MAX_CAMERAS ? s : [...s, id]))

  const win = useMemo(() => {
    if (zoom >= 1440 || position === null) return { start: day, end: dayEnd }
    const half = (zoom * MINUTE) / 2
    const start = Math.max(day, Math.min(dayEnd - 2 * half, position - half))
    return { start, end: start + 2 * half }
  }, [zoom, position, day, dayEnd])

  // Events of the shown cameras, oldest first; the side list shows those of the timeline window.
  const events = useMemo(
    () => selected.flatMap((id) => data.data?.events?.[id] ?? []).sort((a, b) => a.start_ms - b.start_ms),
    [data.data, selected],
  )
  const inWindow = useMemo(() => events.filter((e) => e.start_ms >= win.start && e.start_ms < win.end).reverse(), [events, win])
  // The event being watched: the last one started before the clock.
  const current = position === null ? null : [...events].reverse().find((e) => e.start_ms <= position + LEAD_MS && (e.end_ms ?? e.start_ms) + 30_000 >= position) ?? null
  const playEvent = (e: NvrEvent) => {
    seek(e.start_ms - LEAD_MS)
    setPlaying(true)
  }
  const eventJump = (dir: -1 | 1) => {
    if (position === null) return
    const target = dir > 0 ? events.find((e) => e.start_ms - LEAD_MS > position + 1_000) : [...events].reverse().find((e) => e.start_ms - LEAD_MS < position - 3_000)
    if (target) playEvent(target)
  }

  const noVideo = demo ? t('Mode démo : les enregistrements sont fictifs, il n’y a pas de vidéo.', 'Demo mode: recordings are fictional, there is no video.') : all.length === 0 ? t('Aucun enregistrement ce jour-là.', 'No recording on that day.') : null
  const noEvent = events.length === 0 ? t('Aucun événement ce jour-là sur ces caméras.', 'No event on that day on these cameras.') : null
  const nameOf = (id: string) => cameras.find((c) => c.id === id)?.name ?? id
  const codecOf = (id: string) => cameras.find((c) => c.id === id)?.codec ?? null

  return (
    <>
      <header className="mb-4 flex flex-wrap items-center gap-x-4 gap-y-3">
        <h1 className="text-xl font-semibold tracking-tight">{t('Lecture', 'Playback')}</h1>
        <input
          type="date"
          aria-label={t('Jour', 'Day')}
          className="h-9 rounded-md border border-line bg-bg-2 px-2 text-sm"
          value={toDateInput(day)}
          max={toDateInput(now)}
          onChange={(e) => {
            const d = fromDateInput(e.target.value)
            if (d === null) return
            setDay(d)
            setPosition(null)
            setPlaying(false)
          }}
        />
        <div role="group" aria-label={t(`Caméras affichées (${MAX_CAMERAS} au plus)`, `Shown cameras (${MAX_CAMERAS} at most)`)} className="flex max-w-full gap-1.5 overflow-x-auto">
          {!loaded ? (
            <span className="text-xs text-muted">{t('Chargement…', 'Loading…')}</span>
          ) : (
            cameras.map((c) => {
              const on = selected.includes(c.id)
              const full = !on && selected.length >= MAX_CAMERAS
              return (
                <button
                  key={c.id}
                  type="button"
                  aria-pressed={on}
                  disabled={full}
                  title={full ? t(`${MAX_CAMERAS} caméras au maximum`, `${MAX_CAMERAS} cameras at most`) : undefined}
                  onClick={() => toggleCamera(c.id)}
                  className={chip(on)}
                >
                  {c.name}
                </button>
              )
            })
          )}
          {demo && <Badge tone="demo">{t('Démo', 'Demo')}</Badge>}
        </div>
        <div className="flex flex-wrap gap-2 lg:ml-auto">
          {canBookmark && (
            <Button icon={<BookmarkIcon className="size-4" />} disabledReason={readOnly ?? (position === null ? t('Aucune position de lecture', 'No playback position') : null)} onClick={() => setBookmarking(true)}>
              {t('Signet', 'Bookmark')}
            </Button>
          )}
          {canProtect && (
            <Button icon={<Shield className="size-4" />} disabledReason={noVideo ?? readOnly} onClick={() => setProtecting(true)}>
              {t('Protéger', 'Protect')}
            </Button>
          )}
          <Button
            variant="primary"
            icon={<Download className="size-4" />}
            disabledReason={selected.length === 0 ? t('Choisissez au moins une caméra', 'Choose at least one camera') : noVideo ?? (canExport ? null : t('L’export nécessite FFmpeg sur le serveur', 'Export needs FFmpeg on the server'))}
            onClick={() => setExporting(true)}
          >
            {t('Exporter MP4', 'Export MP4')}
          </Button>
        </div>
      </header>

      {selected.length === 0 ? (
        <Card>
          <EmptyState title={t('Choisissez au moins une caméra', 'Choose at least one camera')} />
        </Card>
      ) : data.error ? (
        <Card>
          <ErrorState error={data.error} onRetry={data.reload} />
        </Card>
      ) : !data.data ? (
        <LoadingState label={t('Chargement des enregistrements…', 'Loading recordings…')} />
      ) : (
        <div className="space-y-4">
          <div className="flex flex-wrap gap-4">
            <div className={`grid min-w-0 flex-[999_1_640px] content-start gap-2 ${selected.length > 1 ? 'grid-cols-2' : ''}`}>
              {selected.map((id) => (
                <RecordingPlayer
                  camera={id}
                  key={id}
                  name={nameOf(id)}
                  codec={codecOf(id)}
                  masks={cameras.find((c) => c.id === id)?.privacy_masks ?? []}
                  width={cameras.find((c) => c.id === id)?.width ?? null}
                  height={cameras.find((c) => c.id === id)?.height ?? null}
                  recordings={recordings[id] ?? []}
                  position={position ?? day}
                  playing={playing}
                  rate={rate}
                  master={id === master}
                  clock={selected.length > 1 && id === master}
                  onTime={id === master ? setPosition : undefined}
                  onGap={id === master ? onGap : undefined}
                />
              ))}
            </div>

            {data.data.events !== null && (
              <Card className="flex min-w-0 flex-[1_1_300px] flex-col lg:max-w-[340px]">
                <h2 className="px-4 pt-3.5 pb-2 text-[15px] font-semibold">{t('Dans cette période', 'In this period')}</h2>
                {inWindow.length === 0 ? (
                  <p className="px-4 pb-3 text-sm text-muted">{t('Aucun événement dans la période affichée.', 'No event in the shown period.')}</p>
                ) : (
                  <ul className="max-h-72 overflow-y-auto pb-2 lg:max-h-[min(60vh,560px)]">
                    {inWindow.map((e) => {
                      const ai = e.objects.length > 0
                      const on = e.id === current?.id
                      return (
                        <li key={e.id}>
                          <button
                            type="button"
                            aria-current={on ? 'true' : undefined}
                            onClick={() => playEvent(e)}
                            className={`flex w-full items-center gap-2.5 px-4 py-2 text-left transition-colors hover:bg-line/40 ${on ? 'bg-line/60' : ''}`}
                          >
                            <span className="w-16 shrink-0 font-mono text-xs text-muted tabular">{formatTime(e.start_ms, true)}</span>
                            <span className={`size-2 shrink-0 rounded-full ${ai ? 'bg-info' : 'bg-motion'}`} aria-hidden />
                            <span className="flex min-w-0 flex-col">
                              <span className="truncate text-[13px] font-semibold">{eventTitle(e)}</span>
                              <span className="truncate text-xs text-muted">{nameOf(e.camera)}</span>
                            </span>
                          </button>
                        </li>
                      )
                    })}
                  </ul>
                )}
                <p className="mt-auto border-t border-line px-4 py-3 text-xs text-muted">
                  {selected.length > 1
                    ? t('Clic sur un événement : les caméras sautent à cet instant, 5 s avant.', 'Click an event: the cameras jump to that moment, 5 s before.')
                    : t('Clic sur un événement : la vidéo saute à cet instant, 5 s avant.', 'Click an event: the video jumps to that moment, 5 s before.')}
                </p>
              </Card>
            )}
          </div>

          <Card className="flex flex-col gap-3 px-4 py-3.5 sm:px-5">
            <div className="flex flex-wrap items-center gap-2.5">
              <button
                type="button"
                aria-label={t('Reculer de 10 s', 'Back 10 s')}
                title={noVideo ?? t('Reculer de 10 s', 'Back 10 s')}
                disabled={noVideo !== null}
                onClick={() => position !== null && seek(position - 10_000)}
                className="size-10 rounded-lg border border-line font-mono text-xs hover:bg-line/40 disabled:opacity-40"
              >
                −10
              </button>
              <button
                type="button"
                aria-label={playing ? t('Pause', 'Pause') : t('Lire', 'Play')}
                title={noVideo ?? (playing ? t('Pause', 'Pause') : t('Lire', 'Play'))}
                disabled={noVideo !== null}
                onClick={() => setPlaying(!playing)}
                className="flex size-12 items-center justify-center rounded-full bg-accent text-on-accent hover:brightness-110 disabled:opacity-40"
              >
                {playing ? <Pause className="size-[18px]" fill="currentColor" aria-hidden /> : <Play className="ml-0.5 size-[18px]" fill="currentColor" aria-hidden />}
              </button>
              <button
                type="button"
                aria-label={t('Avancer de 10 s', 'Forward 10 s')}
                title={noVideo ?? t('Avancer de 10 s', 'Forward 10 s')}
                disabled={noVideo !== null}
                onClick={() => position !== null && seek(position + 10_000)}
                className="size-10 rounded-lg border border-line font-mono text-xs hover:bg-line/40 disabled:opacity-40"
              >
                +10
              </button>
              <span className="mx-1 font-mono text-[22px] tabular" aria-live="off">
                {position !== null ? formatTime(position, true) : '—'}
              </span>
              <Segmented label={t('Vitesse', 'Speed')} options={SPEEDS.map((s) => ({ value: s, label: `×${s}` }))} value={rate} onChange={setRate} />
              <div className="flex gap-1.5">
                <Button size="sm" icon={<ChevronLeft className="size-4" />} disabledReason={noEvent} onClick={() => eventJump(-1)} aria-label={t('Événement précédent', 'Previous event')} title={noEvent ?? t('Événement précédent', 'Previous event')}>
                  <span className="sr-only">{t('Événement précédent', 'Previous event')}</span>
                </Button>
                <Button size="sm" disabledReason={noEvent} onClick={() => eventJump(1)}>
                  {t('Événement suivant', 'Next event')}
                  <ChevronRight className="size-4" aria-hidden />
                </Button>
              </div>
              <div className="flex items-center gap-2 lg:ml-auto">
                <span className="text-xs text-muted">{t('Zoom', 'Zoom')}</span>
                <Segmented label={t('Échelle de la timeline', 'Timeline scale')} options={ZOOMS} value={zoom} onChange={setZoom} />
              </div>
            </div>
            <div className="overflow-x-auto">
              <div className="min-w-[520px]">
                <Timeline
                  rows={selected.map((id) => ({ id, label: nameOf(id) }))}
                  start={win.start}
                  end={win.end}
                  now={now}
                  recordings={data.data.periods}
                  motion={data.data.motion}
                  events={data.data.markers}
                  bookmarks={canBookmark ? bookmarkMarkers : undefined}
                  cursor={position}
                  activeRow={master}
                  onSelect={(_, t) => seek(t)}
                />
              </div>
            </div>
          </Card>
          <ExportDialog
            open={exporting}
            onClose={() => setExporting(false)}
            cameras={selected.map((id) => ({ id, name: nameOf(id), codec: codecOf(id) }))}
            initialCamera={master}
            day={day}
            position={position}
          />
          {canBookmark && (
            <>
              <BookmarkDialog
                open={bookmarking}
                onClose={() => setBookmarking(false)}
                onAdded={bookmarkList.reload}
                cameras={selected.map((id) => ({ id, name: nameOf(id) }))}
                initialCamera={master}
                position={position}
              />
              <BookmarkList
                list={bookmarkList.data ? shownBookmarks : null}
                error={bookmarkList.error}
                onReload={bookmarkList.reload}
                names={nameOf}
                readOnly={readOnly}
                onPlay={(b) => {
                  seek(b.at_ms - LEAD_MS)
                  setPlaying(true)
                  window.scrollTo({ top: 0, behavior: 'smooth' })
                }}
              />
            </>
          )}
          {canProtect && (
            <>
              <ProtectDialog
                open={protecting}
                onClose={() => setProtecting(false)}
                onProtected={protectedList.reload}
                cameras={selected.map((id) => ({ id, name: nameOf(id) }))}
                initialCamera={master}
                day={day}
                position={position}
              />
              <ProtectedList
                list={protectedList.data}
                error={protectedList.error}
                onReload={protectedList.reload}
                names={nameOf}
                readOnly={readOnly}
                onPlay={(p) => {
                  setSelected((s) => (s.includes(p.camera) ? s : [p.camera, ...s].slice(0, MAX_CAMERAS)))
                  setDay(startOfDay(p.start_ms))
                  setPosition(p.start_ms)
                  setPlaying(true)
                  window.scrollTo({ top: 0, behavior: 'smooth' })
                }}
              />
            </>
          )}
          <p className="text-xs text-muted">
            {t('Lecture HLS : les segments qui se suivent sont enchaînés sans pause (hls.js, ou HLS natif sur Safari et iPhone). Les vitesses et le H.265 dépendent du navigateur.', 'HLS playback: consecutive segments play without pause (hls.js, or native HLS on Safari and iPhone). Speeds and H.265 depend on the browser.')}
          </p>
        </div>
      )}
    </>
  )
}
