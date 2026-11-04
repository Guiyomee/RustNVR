import { Activity, ArrowRight, Cpu, HardDrive, Video } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { UnavailableError } from '../api/client'
import { CameraGrid, type GridColumns } from '../components/cameras/CameraCard'
import { EventRow } from '../components/events/EventCard'
import { StorageCard } from '../components/storage/StorageCard'
import { TimelineCard } from '../components/timeline/TimelineCard'
import { Button } from '../components/ui/Button'
import { Card, CardHeader } from '../components/ui/Card'
import { Meter } from '../components/ui/Meter'
import { Sparkline } from '../components/ui/Sparkline'
import { StatCard } from '../components/ui/StatCard'
import { Segmented } from '../components/ui/Tabs'
import { EmptyState, ErrorState, LoadingState, Unavailable } from '../components/ui/States'
import { useLive } from '../data/DataContext'
import { bucketByHour, useEvents } from '../hooks/useEvents'
import { diskPercent, isRemote, useStorage } from '../hooks/useStorage'
import { useNow } from '../hooks/useUi'
import { formatBytes, formatNumber, formatPercent } from '../lib/format'
import { Link, navigate } from '../lib/router'
import type { Camera } from '../types/api'
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

const ORDER_KEY = 'rustnvr.overview.order'

/** Order of the cameras on the overview (identifiers), remembered by the browser; empty: default order. */
function useCameraOrder(): [string[], (ids: string[]) => void] {
  const [order, setOrder] = useState<string[]>(() => {
    try {
      const stored: unknown = JSON.parse(window.localStorage.getItem(ORDER_KEY) ?? '[]')
      return Array.isArray(stored) ? stored.filter((x): x is string => typeof x === 'string') : []
    } catch {
      return []
    }
  })
  return [
    order,
    (ids) => {
      if (ids.length === 0) window.localStorage.removeItem(ORDER_KEY)
      else window.localStorage.setItem(ORDER_KEY, JSON.stringify(ids))
      setOrder(ids)
    },
  ]
}

/** `cameras` in the chosen order; cameras not placed yet keep their default order, at the end. */
function ordered(cameras: Camera[], order: string[]): Camera[] {
  const rank = new Map(order.map((id, i) => [id, i]))
  return [...cameras].sort((a, b) => (rank.get(a.id) ?? 1e9) - (rank.get(b.id) ?? 1e9))
}

/** Number of online cameras sampled every 10 s since the page was opened. */
function useOnlineHistory(online: number): number[] {
  const [history, setHistory] = useState<number[]>([])
  const latest = useRef(online)
  latest.current = online
  useEffect(() => {
    const push = () => setHistory((h) => [...h.slice(-29), latest.current])
    push()
    const id = window.setInterval(push, 10_000)
    return () => window.clearInterval(id)
  }, [])
  return history
}

function RecentEvents({ cameras }: { cameras: Camera[] }) {
  const now = useNow(60_000)
  const query = useMemo(() => ({ camera: null, fromMs: now - DAY, toMs: now, kinds: null }), [now])
  const events = useEvents(query)
  const names = useMemo(() => new Map(cameras.map((c) => [c.id, c.name])), [cameras])
  return (
    <Card className="flex min-h-0 flex-col">
      <CardHeader
        title={t('Événements récents', 'Recent events')}
        icon={<Activity className="size-4" />}
        actions={
          <Button size="sm" variant="ghost" onClick={() => navigate('/events')}>
            {t('Tout voir ', 'See all ')}<ArrowRight className="size-3.5" aria-hidden />
          </Button>
        }
      />
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

function EventsStat() {
  const now = useNow(60_000)
  const query = useMemo(() => ({ camera: null, fromMs: now - DAY, toMs: now, kinds: null }), [now])
  const events = useEvents(query)
  if (events.error instanceof UnavailableError) {
    return <StatCard label={t('Événements (24 h)', 'Events (24 h)')} icon={<Activity className="size-4" />} value="—" detail={t('Détection non disponible', 'Detection not available')} tone="neutral" />
  }
  const list = events.data ?? []
  return (
    <StatCard label={t('Événements (24 h)', 'Events (24 h)')} icon={<Activity className="size-4" />} value={events.data ? formatNumber(list.length) : '…'} detail={t('Mouvements détectés', 'Detected motion')} tone="info">
      <Sparkline values={bucketByHour(list, 12, now)} min={0} label={t('Événements par heure, 12 dernières heures', 'Events per hour, last 12 hours')} format={(v) => `${v} / h`} width={96} />
    </StatCard>
  )
}

export function OverviewPage() {
  const cameras = useLive((s) => s.cameras)
  const loaded = useLive((s) => s.loaded)
  const error = useLive((s) => s.error)
  const system = useLive((s) => s.system)
  const storage = useStorage()
  const [order, setOrder] = useCameraOrder()
  const enabled = ordered(
    cameras.filter((c) => c.enabled),
    order,
  )
  const online = enabled.filter((c) => c.status === 'online').length
  const onlineHistory = useOnlineHistory(online)
  // Disk of the default destination (where cameras record unless told
  // otherwise); the recordings directory's disk for S3 / SFTP or before the
  // server reports it.
  const defaultDest = storage.data?.destinations.find((d) => d.id === storage.data?.default_destination && !isRemote(d))
  const shownDisk = defaultDest ? defaultDest.disk : storage.data?.disk
  const diskPct = diskPercent(shownDisk)
  // Rows only change when cameras are added/renamed, not on every live update.
  const rowsKey = JSON.stringify(enabled.map((c) => [c.id, c.name]))
  const rows = useMemo(() => (JSON.parse(rowsKey) as [string, string][]).map(([id, label]) => ({ id, label })), [rowsKey])
  const [columns, setColumns] = useColumns()
  // With 3 or 4 cameras per row, the cameras take the full width and the side column goes below.
  const wide = columns === 3 || columns === 4

  return (
    <div className="space-y-4 lg:space-y-5">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <StatCard
          label={t('Caméras actives', 'Active cameras')}
          icon={<Video className="size-4" />}
          value={loaded ? `${online} / ${enabled.length}` : '…'}
          detail={loaded ? (online === enabled.length ? t('Toutes en ligne', 'All online') : t(`${enabled.length - online} hors ligne`, `${enabled.length - online} offline`)) : t('Connexion…', 'Connecting…')}
          tone={!loaded ? 'neutral' : online === enabled.length ? 'ok' : online === 0 ? 'err' : 'warn'}
        >
          <Sparkline values={onlineHistory} min={0} max={Math.max(1, enabled.length)} label={t("Caméras en ligne depuis l'ouverture de la page", "Cameras online since the page was opened")} format={(v) => t(`${v} en ligne`, `${v} online`)} width={96} />
        </StatCard>
        <EventsStat />
        <StatCard
          label={defaultDest ? t(`Stockage · ${defaultDest.name}`, `Storage · ${defaultDest.name}`) : t('Stockage', 'Storage')}
          icon={<HardDrive className="size-4" />}
          value={diskPct === null ? '—' : formatPercent(diskPct)}
          detail={shownDisk ? `${formatBytes(shownDisk.used_bytes)} / ${formatBytes(shownDisk.total_bytes)}` : defaultDest && !defaultDest.available ? t('Point de stockage indisponible', 'Destination unavailable') : storage.error ? t('Indisponible', 'Unavailable') : '…'}
          tone={(defaultDest && !defaultDest.available) || (diskPct !== null && diskPct >= 90) ? 'err' : diskPct !== null && diskPct >= 75 ? 'warn' : 'info'}
        >
          <div className="w-24 pb-1.5">
            <Meter value={diskPct} label={defaultDest ? t(`Occupation de ${defaultDest.name}`, `${defaultDest.name} usage`) : t('Occupation du disque', 'Disk usage')} />
          </div>
        </StatCard>
        <StatCard
          label="CPU"
          icon={<Cpu className="size-4" />}
          value={formatPercent(system?.cpu_percent)}
          detail={(() => {
            const own = system?.service_cpu_percent ?? system?.process_cpu_percent
            return own != null ? `RustNVR ${formatNumber(own, 1)} %` : t('Machine, tous cœurs', 'Machine, all cores')
          })()}
          title={t("Grand chiffre : toute la machine, tous cœurs (sous Docker : la machine hôte). RustNVR : avec les FFmpeg qu'il lance, en % d'un cœur comme docker stats.", 'Large figure: the whole machine, all cores (under Docker: the host). RustNVR: with the FFmpeg processes it starts, as % of one core like docker stats.')}
          tone={(system?.cpu_percent ?? 0) >= 90 ? 'err' : (system?.cpu_percent ?? 0) >= 75 ? 'warn' : 'info'}
        >
          <Sparkline values={system?.cpu_history ?? []} min={0} max={100} label={t('CPU sur la dernière minute', 'CPU over the last minute')} format={(v) => `${formatNumber(v)} %`} width={96} />
        </StatCard>
      </div>

      <div className={`grid grid-cols-1 gap-4 lg:gap-5 ${wide ? '' : 'xl:grid-cols-[minmax(0,1fr)_340px]'}`}>
        <section aria-labelledby="cams-title" className="min-w-0">
          <div className="mb-3 flex items-center justify-between">
            <h2 id="cams-title" className="text-sm font-semibold">
              {t('Caméras', 'Cameras')}
            </h2>
            <div className="flex items-center gap-3">
              {order.length > 0 && (
                <Button size="sm" variant="ghost" title={t("Revenir à l'ordre des caméras de la configuration", "Back to the camera order of the configuration")} onClick={() => setOrder([])}>
                  {t('Ordre par défaut', 'Default order')}
                </Button>
              )}
              <span className="hidden items-center gap-2 text-xs text-muted md:flex">
                {t('Par ligne', 'Per row')}
                <Segmented label={t('Caméras par ligne', 'Cameras per row')} options={COLUMN_OPTIONS} value={columns} onChange={setColumns} />
              </span>
              <Link to="/cameras" className="text-xs text-accent hover:underline">
                {t('Gérer les caméras', 'Manage cameras')}
              </Link>
            </div>
          </div>
          {!loaded ? (
            error ? <ErrorState error={error} /> : <LoadingState />
          ) : enabled.length === 0 ? (
            <Card>
              <EmptyState title={t('Aucune caméra activée', 'No camera on')} icon={<Video className="size-7" />}>
                {t('Déclarez vos caméras dans config.toml (section [[cameras]]), puis redémarrez RustNVR.', 'Declare your cameras in config.toml ([[cameras]] section), then restart RustNVR.')}
              </EmptyState>
            </Card>
          ) : (
            <CameraGrid cameras={enabled} columns={columns} onReorder={setOrder} />
          )}
        </section>
        <aside className={`grid min-w-0 gap-4 lg:gap-5 ${wide ? 'lg:grid-cols-2' : 'grid-cols-1 content-start'}`}>
          <RecentEvents cameras={cameras} />
          <StorageCard storage={storage.data} error={storage.error} onRetry={storage.reload} />
        </aside>
      </div>

      {rows.length > 0 && <TimelineCard rows={rows} onSelect={(camera, t) => navigate(`/playback?camera=${encodeURIComponent(camera)}&t=${Math.round(t)}`)} />}
    </div>
  )
}
