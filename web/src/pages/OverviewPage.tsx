import { Activity, ArrowRight, MonitorPlay, Video } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { describeError, UnavailableError } from '../api/client'
import { getCameraOrder, setCameraOrder } from '../api/endpoints'
import { CameraGrid, type GridColumns } from '../components/cameras/CameraCard'
import { PhoneCameras } from '../components/cameras/PhoneCameras'
import { EventRow } from '../components/events/EventCard'
import { StorageCard } from '../components/storage/StorageCard'
import { TimelineCard } from '../components/timeline/TimelineCard'
import { Button } from '../components/ui/Button'
import { Card, CardHeader } from '../components/ui/Card'
import { useIsViewer } from '../auth/AuthGate'
import { Segmented } from '../components/ui/Tabs'
import { EmptyState, ErrorState, LoadingState, Unavailable } from '../components/ui/States'
import { useIsDemo, useLive } from '../data/DataContext'
import { useAsync } from '../hooks/useAsync'
import { useEvents } from '../hooks/useEvents'
import { useStorage } from '../hooks/useStorage'
import { useMediaQuery, useNow } from '../hooks/useUi'
import { formatNumber } from '../lib/format'
import { Link, navigate } from '../lib/router'
import type { Camera, EventKind } from '../types/api'
import { t } from '../lib/i18n'

const DAY = 86_400_000

const COLUMNS_KEY = 'rustnvr.overview.columns'
const COLUMN_OPTIONS: { value: GridColumns; label: string }[] = [
  { value: 'auto', label: 'Auto' },
  { value: 1, label: '1' },
  { value: 2, label: '2' },
  { value: 3, label: '3' },
  { value: 4, label: '4' },
]

/** Cameras per row on the overview, remembered by the browser. */
function useColumns(): [GridColumns, (c: GridColumns) => void] {
  const [columns, setColumns] = useState<GridColumns>(() => {
    const stored = window.localStorage.getItem(COLUMNS_KEY)
    return COLUMN_OPTIONS.find((o) => String(o.value) === stored)?.value ?? 'auto'
  })
  return [
    columns,
    (c) => {
      window.localStorage.setItem(COLUMNS_KEY, String(c))
      setColumns(c)
    },
  ]
}

/** Where versions before 2026.11.08 kept the order, in each browser. */
const OLD_ORDER_KEY = 'rustnvr.overview.order'

function oldOrder(): string[] {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(OLD_ORDER_KEY) ?? '[]')
    return Array.isArray(stored) ? stored.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

/**
 * Order of the cameras, kept on the server: the same on every device and
 * for every account (the camera list already comes in that order). A drag
 * shows the new order at once, until the server's list follows. Only
 * administrators change it; the demo keeps it on the page.
 */
function useCameraOrder(cameras: Camera[], canChange: boolean) {
  const demo = useIsDemo()
  const saved = useAsync((signal) => (demo ? Promise.resolve({ cameras: [] as string[] }) : getCameraOrder(signal)), [demo])
  const [pending, setPending] = useState<string[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const serverKey = cameras.map((c) => c.id).join('/')
  // The server's list changed (saved order, camera added): it is the truth again.
  useEffect(() => {
    if (!demo) setPending(null)
  }, [serverKey, demo])
  const save = (ids: string[]) => {
    setPending(ids)
    setError(null)
    if (demo) return
    setCameraOrder(ids).then(saved.reload, (e: unknown) => {
      setPending(null)
      setError(describeError(e))
    })
  }
  // The order a browser kept before, sent once to the server if none is set.
  useEffect(() => {
    const old = oldOrder()
    if (old.length === 0 || demo || !canChange || !saved.data) return
    try {
      window.localStorage.removeItem(OLD_ORDER_KEY)
    } catch {
      // Storage blocked: nothing to clean.
    }
    if (saved.data.cameras.length === 0) save(old)
  }, [saved.data, canChange, demo])
  const custom = pending ? pending.length > 0 : (saved.data?.cameras.length ?? 0) > 0
  return { shown: pending ? ordered(cameras, pending) : cameras, custom, save, error }
}

/** `cameras` in the chosen order; cameras not placed yet keep their default order, at the end. */
function ordered(cameras: Camera[], order: string[]): Camera[] {
  const rank = new Map(order.map((id, i) => [id, i]))
  return [...cameras].sort((a, b) => (rank.get(a.id) ?? 1e9) - (rank.get(b.id) ?? 1e9))
}

const EVENT_FILTERS: { kind: EventKind | null; label: string }[] = [
  { kind: null, label: t('Tous', 'All') },
  { kind: 'person', label: t('Personne', 'Person') },
  { kind: 'vehicle', label: t('Véhicule', 'Vehicle') },
  { kind: 'animal', label: t('Animal', 'Animal') },
  { kind: 'motion', label: t('Mouvement', 'Motion') },
]

function RecentEvents({ cameras }: { cameras: Camera[] }) {
  const now = useNow(60_000)
  const [kind, setKind] = useState<EventKind | null>(null)
  const query = useMemo(() => ({ camera: null, fromMs: now - DAY, toMs: now, kinds: kind ? [kind] : null }), [now, kind])
  const events = useEvents(query)
  const names = useMemo(() => new Map(cameras.map((c) => [c.id, c.name])), [cameras])
  return (
    <Card className="flex min-h-0 flex-col">
      <CardHeader
        title={t('Événements', 'Events')}
        subtitle={events.data ? t(`${formatNumber(events.data.length)} sur 24 h`, `${formatNumber(events.data.length)} in 24 h`) : undefined}
        icon={<Activity className="size-4" />}
        actions={
          <Button size="sm" variant="ghost" onClick={() => navigate('/events')}>
            {t('Tout voir ', 'See all ')}<ArrowRight className="size-3.5" aria-hidden />
          </Button>
        }
      />
      <div role="group" aria-label={t('Filtrer les événements', 'Filter the events')} className="flex gap-1 overflow-x-auto border-b border-line px-3 py-2.5">
        {EVENT_FILTERS.map((f) => {
          const on = kind === f.kind
          return (
            <button
              key={f.label}
              type="button"
              aria-pressed={on}
              onClick={() => setKind(f.kind)}
              className={`h-8 shrink-0 rounded-full border px-2.5 text-xs ${on ? 'border-accent bg-accent/15 text-accent' : 'border-line text-muted hover:text-fg'}`}
            >
              {f.label}
            </button>
          )
        })}
      </div>
      {events.error instanceof UnavailableError ? (
        <Unavailable compact title={t('Événements indisponibles', 'Events unavailable')}>
          {t("Base de données indisponible sur le serveur : aucun événement n'est enregistré.", "Database unavailable on the server: no event is recorded.")}
        </Unavailable>
      ) : events.error ? (
        <ErrorState compact error={events.error} onRetry={events.reload} />
      ) : !events.data ? (
        <LoadingState compact />
      ) : events.data.length === 0 ? (
        <EmptyState title={t('Aucun événement sur 24 h', 'No event in 24 h')}>{t("Activez la détection dans l'onglet « Détection » d'une caméra.", "Turn detection on in the « Detection » tab of a camera.")}</EmptyState>
      ) : (
        <ul className="divide-y divide-line overflow-y-auto xl:max-h-[420px]">
          {events.data.slice(0, 8).map((e) => (
            <EventRow
              key={e.id}
              event={e}
              cameraName={names.get(e.camera) ?? e.camera}
              onClick={() => navigate(`/playback?camera=${encodeURIComponent(e.camera)}&t=${e.start_ms}`)}
            />
          ))}
        </ul>
      )}
    </Card>
  )
}

export function OverviewPage() {
  const viewer = useIsViewer()
  const cameras = useLive((s) => s.cameras)
  const loaded = useLive((s) => s.loaded)
  const error = useLive((s) => s.error)
  const storage = useStorage()
  const order = useCameraOrder(cameras, !viewer)
  const enabled = order.shown.filter((c) => c.enabled)
  // Rows only change when cameras are added/renamed, not on every live update.
  const rowsKey = JSON.stringify(enabled.map((c) => [c.id, c.name]))
  const rows = useMemo(() => (JSON.parse(rowsKey) as [string, string][]).map(([id, label]) => ({ id, label })), [rowsKey])
  const [columns, setColumns] = useColumns()
  // Phones: one camera large, the others small (see PhoneCameras).
  const phone = !useMediaQuery('(min-width: 768px)')

  return (
    <div className="grid grid-cols-1 gap-5 xl:grid-cols-[minmax(0,1fr)_360px]">
      <section aria-labelledby="cams-title" className="flex min-w-0 flex-col gap-4">
        <div className="flex flex-wrap items-center gap-3">
          <h2 id="cams-title" className="text-[15px] font-semibold">
            {t('Caméras', 'Cameras')}
          </h2>
          {order.custom && !viewer && (
            <Button size="sm" variant="ghost" title={t("Revenir à l'ordre des caméras de la configuration, pour tous les appareils", 'Back to the camera order of the configuration, on every device')} onClick={() => order.save([])}>
              {t('Ordre par défaut', 'Default order')}
            </Button>
          )}
          <span className="flex-1" />
          <span className="hidden items-center gap-2 text-xs text-muted md:flex">
            {t('Par ligne', 'Per row')}
            <Segmented label={t('Caméras par ligne', 'Cameras per row')} options={COLUMN_OPTIONS} value={columns} onChange={setColumns} />
          </span>
          {!viewer && (
            <Link to="/cameras" className="text-xs text-accent hover:underline">
              {t('Gérer les caméras', 'Manage cameras')}
            </Link>
          )}
          <Link to="/mur" className="hidden h-9 items-center gap-2 sm:inline-flex rounded-lg bg-accent px-3.5 text-[13px] font-semibold text-on-accent hover:bg-accent/90">
            <MonitorPlay className="size-4" aria-hidden />
            {t("Ouvrir le mur d'écrans", 'Open the video wall')}
          </Link>
        </div>
        {!loaded ? (
          error ? <ErrorState error={error} /> : <LoadingState />
        ) : enabled.length === 0 ? (
          <Card>
            <EmptyState title={t('Aucune caméra activée', 'No camera on')} icon={<Video className="size-7" />}>
              {t('Ajoutez une caméra dans Caméras → Ajouter une caméra, ou dans config.toml (section [[cameras]]).', 'Add a camera in Cameras → Add a camera, or in config.toml ([[cameras]] section).')}
            </EmptyState>
          </Card>
        ) : (
          <>
            {order.error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{t("L'ordre n'a pas été enregistré : ", 'The order was not saved: ')}{order.error}</p>}
            {phone ? (
              <PhoneCameras cameras={enabled} />
            ) : (
              <CameraGrid cameras={enabled} columns={columns} onReorder={viewer ? undefined : (ids) => order.save([...ids, ...order.shown.filter((c) => !c.enabled).map((c) => c.id)])} />
            )}
          </>
        )}
        {rows.length > 0 && !phone && <TimelineCard rows={rows} onSelect={(camera, at) => navigate(`/playback?camera=${encodeURIComponent(camera)}&t=${Math.round(at)}`)} />}
      </section>
      <aside aria-label={t('Événements et stockage', 'Events and storage')} className="flex min-w-0 flex-col gap-4">
        <RecentEvents cameras={cameras} />
        <StorageCard storage={storage.data} error={storage.error} onRetry={storage.reload} />
      </aside>
    </div>
  )
}
