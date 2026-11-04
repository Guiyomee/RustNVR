import { Download, FastForward, Pause, Play, Rewind, Shield, SkipBack, SkipForward } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState } from 'react'
import { ExportDialog } from '../components/playback/ExportDialog'
import { ProtectDialog, ProtectedList } from '../components/playback/ProtectDialog'
import { RecordingPlayer } from '../components/playback/RecordingPlayer'
import { WINDOW_OPTIONS, type WindowHours } from '../components/timeline/TimelineCard'
import { Timeline } from '../components/timeline/Timeline'
import { useTimelineData } from '../components/timeline/useTimelineData'
import { Button, IconButton } from '../components/ui/Button'
import { Badge, Card, CardHeader, PageHeader } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { Segmented } from '../components/ui/Tabs'
import { useCapability, useIsDemo, useLive } from '../data/DataContext'
import { getProtected } from '../api/endpoints'
import { useReadOnly } from '../auth/AuthGate'
import { useAsync, useInterval } from '../hooks/useAsync'
import { useNow } from '../hooks/useUi'
import { formatDateTime, fromDateInput, startOfDay, toDateInput } from '../lib/format'
import { navigate, useLocation } from '../lib/router'
import type { Recording } from '../types/api'
import { t } from '../lib/i18n'

const DAY = 86_400_000
const HOUR = 3_600_000
const MAX_CAMERAS = 4
const SPEEDS = [1, 2, 4, 8] as const
type Speed = (typeof SPEEDS)[number]

function sortedAll(recs: Record<string, Recording[]>, ids: string[]): Recording[] {
  return ids.flatMap((id) => recs[id] ?? []).sort((a, b) => a.start_ms - b.start_ms)
}

export function PlaybackPage() {
  const { query } = useLocation()
  const cameras = useLive((s) => s.cameras)
  const loaded = useLive((s) => s.loaded)
  const demo = useIsDemo()
  const now = useNow(30_000)

  const initialT = Number(query.get('t')) || null
  const [selected, setSelected] = useState<string[]>(() => {
    const c = query.get('camera')
    return c ? [c] : []
  })
  const [day, setDay] = useState(() => startOfDay(initialT ?? Date.now()))
  const [position, setPosition] = useState<number | null>(initialT)
  const [playing, setPlaying] = useState(Boolean(initialT))
  const [rate, setRate] = useState<Speed>(1)
  const [zoom, setZoom] = useState<WindowHours>(1)
  const [exporting, setExporting] = useState(false)
  const canExport = useCapability('export') === true
  const canProtect = useCapability('protect') === true && !demo
  const readOnly = useReadOnly()
  const [protecting, setProtecting] = useState(false)
  const protectedList = useAsync(() => (canProtect ? getProtected() : Promise.resolve([])), [canProtect])

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
  const segmentJump = (dir: -1 | 1) => {
    if (position === null) return
    const target = dir > 0 ? all.find((r) => r.start_ms > position + 500) : [...all].reverse().find((r) => r.start_ms < position - 2000)
    if (target) seek(target.start_ms)
  }

  const toggleCamera = (id: string) =>
    setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : s.length >= MAX_CAMERAS ? s : [...s, id]))

  const win = useMemo(() => {
    if (zoom === 24 || position === null) return { start: day, end: dayEnd }
    const half = (zoom * HOUR) / 2
    const start = Math.max(day, Math.min(dayEnd - 2 * half, position - half))
    return { start, end: start + 2 * half }
  }, [zoom, position, day, dayEnd])

  const noVideo = demo ? t('Mode démo : les enregistrements sont fictifs, il n’y a pas de vidéo.', 'Demo mode: recordings are fictional, there is no video.') : all.length === 0 ? t('Aucun enregistrement ce jour-là.', 'No recording on that day.') : null
  const nameOf = (id: string) => cameras.find((c) => c.id === id)?.name ?? id
  const codecOf = (id: string) => cameras.find((c) => c.id === id)?.codec ?? null

  return (
    <>
      <PageHeader
        title={t('Lecture', 'Playback')}
        description={t('Relecture des enregistrements continus. Plusieurs caméras sont lues sur la même horloge.', 'Playback of continuous recordings. Several cameras are played on the same clock.')}
        actions={
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
        }
      />

      <Card className="mb-4 flex flex-wrap items-center gap-2 p-3">
        <span className="mr-1 text-xs text-muted">{t('Caméras (', 'Cameras (')}{selected.length}/{MAX_CAMERAS})</span>
        {!loaded ? (
          <span className="text-xs text-muted">{t('Chargement…', 'Loading…')}</span>
        ) : (
          cameras.map((c) => {
            const on = selected.includes(c.id)
            return (
              <button
                key={c.id}
                type="button"
                aria-pressed={on}
                disabled={!on && selected.length >= MAX_CAMERAS}
                title={!on && selected.length >= MAX_CAMERAS ? t(`${MAX_CAMERAS} caméras au maximum`, `${MAX_CAMERAS} cameras at most`) : undefined}
                onClick={() => toggleCamera(c.id)}
                className={`rounded-full border px-3 py-1 text-xs font-medium transition-colors disabled:opacity-40 ${on ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted hover:text-fg'}`}
              >
                {c.name}
              </button>
            )
          })
        )}
        {demo && <Badge tone="demo">{t('Démo', 'Demo')}</Badge>}
      </Card>

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
          <div className={`grid gap-3 ${selected.length > 1 ? 'md:grid-cols-2' : ''}`}>
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
                onTime={id === master ? setPosition : undefined}
                onGap={id === master ? onGap : undefined}
              />
            ))}
          </div>

          <Card>
            <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 py-3">
              <IconButton label={t('Segment précédent', 'Previous segment')} icon={<SkipBack className="size-4" />} disabledReason={noVideo} onClick={() => segmentJump(-1)} />
              <IconButton label={t('Reculer de 10 s', 'Back 10 s')} icon={<Rewind className="size-4" />} disabledReason={noVideo} onClick={() => position !== null && seek(position - 10_000)} />
              <Button
                variant="primary"
                icon={playing ? <Pause className="size-4" /> : <Play className="size-4" />}
                disabledReason={noVideo}
                onClick={() => setPlaying(!playing)}
              >
                {playing ? t('Pause', 'Pause') : t('Lecture', 'Playback')}
              </Button>
              <IconButton label={t('Avancer de 10 s', 'Forward 10 s')} icon={<FastForward className="size-4" />} disabledReason={noVideo} onClick={() => position !== null && seek(position + 10_000)} />
              <IconButton label={t('Segment suivant', 'Next segment')} icon={<SkipForward className="size-4" />} disabledReason={noVideo} onClick={() => segmentJump(1)} />
              <Segmented label={t('Vitesse', 'Speed')} options={SPEEDS.map((s) => ({ value: s, label: `×${s}` }))} value={rate} onChange={setRate} />
              <Button
                icon={<Download className="size-4" />}
                disabledReason={noVideo ?? (canExport ? null : t('L’export nécessite FFmpeg sur le serveur', 'Export needs FFmpeg on the server'))}
                onClick={() => setExporting(true)}
              >
                {t('Exporter', 'Export')}
              </Button>
              {canProtect && (
                <Button icon={<Shield className="size-4" />} disabledReason={noVideo ?? readOnly} onClick={() => setProtecting(true)}>
                  {t('Protéger', 'Protect')}
                </Button>
              )}
              <span className="tabular ml-auto text-sm">{position !== null ? formatDateTime(position) : '—'}</span>
            </div>
            <CardHeader
              title="Timeline"
              subtitle={t('Cliquez pour vous positionner', 'Click to move there')}
              actions={<Segmented label={t('Zoom', 'Zoom')} options={WINDOW_OPTIONS} value={zoom} onChange={setZoom} />}
            />
            <div className="overflow-x-auto px-4 py-4">
              <div className="min-w-[520px]">
                <Timeline
                  rows={selected.map((id) => ({ id, label: nameOf(id) }))}
                  start={win.start}
                  end={win.end}
                  now={now}
                  recordings={data.data.periods}
                  motion={data.data.motion}
                  events={data.data.markers}
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
