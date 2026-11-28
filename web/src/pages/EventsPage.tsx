import { BarChart3, LayoutGrid, List, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { describeError, UnavailableError } from '../api/client'
import { ActivityStats } from '../components/events/ActivityStats'
import { EVENT_KIND_LABEL, EventCard, EventRow, JourneyCard, JourneyRow, groupJourneys, journeyPlaybackUrl, type EventItem } from '../components/events/EventCard'
import { Button } from '../components/ui/Button'
import { Card, PageHeader } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState, Unavailable } from '../components/ui/States'
import { Segmented } from '../components/ui/Tabs'
import { useCapability, useIsDemo, useLive } from '../data/DataContext'
import { deleteEvent, getMotion } from '../api/endpoints'
import { useAuth, useReadOnly } from '../auth/AuthGate'
import { ConfirmDialog } from '../components/ui/Modal'
import { formatTime } from '../lib/format'
import { useAsync } from '../hooks/useAsync'
import { useEvents } from '../hooks/useEvents'
import { useNow, useStoredFlag } from '../hooks/useUi'
import { navigate } from '../lib/router'
import type { EventKind, NvrEvent } from '../types/api'
import { t } from '../lib/i18n'

const PERIODS = [
  { value: 1, label: '1 h' },
  { value: 6, label: '6 h' },
  { value: 24, label: '24 h' },
  { value: 168, label: t('7 j', '7 d') },
  { value: 720, label: t('30 j', '30 d') },
] as const
type PeriodHours = (typeof PERIODS)[number]['value']
const PAGE = 60

const KINDS: { kind: EventKind; needsAi: boolean }[] = [
  { kind: 'motion', needsAi: false },
  { kind: 'person', needsAi: true },
  { kind: 'vehicle', needsAi: true },
  { kind: 'animal', needsAi: true },
]

export function EventsPage() {
  const cameras = useLive((s) => s.cameras)
  const ai = useCapability('ai')
  const now = useNow(60_000)
  const [camera, setCamera] = useState<string>('')
  const [hours, setHours] = useState<PeriodHours>(24)
  const [kinds, setKinds] = useState<EventKind[]>(['motion'])
  const [listView, setListView] = useStoredFlag('rustnvr.events.list', false)
  const [showStats, setShowStats] = useStoredFlag('rustnvr.events.stats', true)
  const demo = useIsDemo()
  const [limit, setLimit] = useState(PAGE)
  const [zone, setZone] = useState('')
  // Plate searched, sent once typing stops.
  const [plateInput, setPlateInput] = useState('')
  const [plate, setPlate] = useState('')
  useEffect(() => {
    const id = setTimeout(() => (setPlate(plateInput.toUpperCase().replace(/[^A-Z0-9]/g, '')), setLimit(PAGE)), 400)
    return () => clearTimeout(id)
  }, [plateInput])
  // Zones of the chosen camera, to search its events by zone.
  const motion = useAsync(() => (camera && !demo ? getMotion(camera).catch(() => null) : Promise.resolve(null)), [camera, demo])
  const zoneNames = motion.data?.zones.map((z) => z.name) ?? []
  const query = useMemo(() => ({ camera: camera || null, fromMs: now - hours * 3_600_000, toMs: now, kinds, zones: zone ? [zone] : null, plate: plate || null }), [camera, hours, kinds, now, zone, plate])
  const events = useEvents(query)
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && !demo && (!user || user.role === 'admin')
  const [toDelete, setToDelete] = useState<NvrEvent | null>(null)
  const [deleteError, setDeleteError] = useState<string | null>(null)
  const removeEvent = async (e: NvrEvent) => {
    setToDelete(null)
    setDeleteError(null)
    try {
      await deleteEvent(e.id, false)
      events.reload()
    } catch (err) {
      setDeleteError(describeError(err))
    }
  }
  /** The trash of an event (administrators; not while it is going on). */
  const trash = (e: NvrEvent, className: string) =>
    admin && (
      <button
        type="button"
        disabled={e.end_ms === null}
        aria-label={t(`Supprimer l’événement de ${formatTime(e.start_ms)}`, `Delete the event of ${formatTime(e.start_ms)}`)}
        title={e.end_ms === null ? t('Événement en cours : attendez sa fin.', 'Event still going on: wait for its end.') : t('Supprimer cet événement (la vidéo reste, voir Vidéos)', 'Delete this event (the video stays, see Videos)')}
        onClick={() => setToDelete(e)}
        className={`z-10 flex size-8 items-center justify-center rounded-md bg-black/60 text-white opacity-0 transition-opacity group-hover:opacity-100 hover:bg-err focus-visible:opacity-100 disabled:cursor-not-allowed disabled:hover:bg-black/60 ${className}`}
      >
        <Trash2 className="size-4" aria-hidden />
      </button>
    )
  const names = useMemo(() => new Map(cameras.map((c) => [c.id, c.name])), [cameras])
  const open = (cam: string, t: number) => navigate(`/playback?camera=${encodeURIComponent(cam)}&t=${t}`)

  if (events.error instanceof UnavailableError) {
    return (
      <>
        <PageHeader title={t('Événements', 'Events')} />
        <Card>
          <Unavailable title={t('Aucun événement disponible', 'No event available')} route="GET /api/v1/events">
            {t("Ce serveur n'enregistre pas d'événements : sa base de données n'a pas pu être ouverte au démarrage (voir le journal de RustNVR). Les enregistrements continus restent consultables dans « Lecture ».", "This server does not record events: its database could not be opened at startup (see the RustNVR log). Continuous recordings can still be watched in « Playback ».")}
          </Unavailable>
        </Card>
      </>
    )
  }

  const list = events.data ?? []
  // The events of a journey together, when every camera is shown.
  const items: EventItem[] = camera ? list.map((event) => ({ kind: 'event', event })) : groupJourneys(list)
  const name = (id: string) => names.get(id) ?? id
  return (
    <>
      <PageHeader
        title={t('Événements', 'Events')}
        description={t("Mouvements détectés. L'IA est optionnelle et désactivée par défaut.", "Detected motion. AI is optional and off by default.")}
        actions={
          <div className="flex items-center gap-2">
          {!demo && (
            <Button size="sm" variant={showStats ? 'primary' : 'secondary'} icon={<BarChart3 className="size-3.5" />} onClick={() => setShowStats(!showStats)} aria-pressed={showStats}>
              {t('Activité', 'Activity')}
            </Button>
          )}
          <div className="flex rounded-md border border-line bg-bg-2 p-0.5">
            <Button size="sm" variant={listView ? 'ghost' : 'primary'} icon={<LayoutGrid className="size-3.5" />} onClick={() => setListView(false)} aria-pressed={!listView}>
              {t('Grille', 'Grid')}
            </Button>
            <Button size="sm" variant={listView ? 'primary' : 'ghost'} icon={<List className="size-3.5" />} onClick={() => setListView(true)} aria-pressed={listView}>
              {t('Liste', 'List')}
            </Button>
          </div>
          </div>
        }
      />
      {showStats && !demo && <ActivityStats names={(id) => names.get(id) ?? id} />}
      <Card className="mb-4 flex flex-wrap items-center gap-4 p-3">
        <label className="flex items-center gap-2 text-sm">
          <span className="text-muted">{t('Caméra', 'Camera')}</span>
          <select className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm" value={camera} onChange={(e) => (setCamera(e.target.value), setZone(''), setLimit(PAGE))}>
            <option value="">{t('Toutes', 'All')}</option>
            {cameras.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
        {zoneNames.length > 0 && (
          <label className="flex items-center gap-2 text-sm">
            <span className="text-muted">{t('Zone', 'Zone')}</span>
            <select className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm" value={zone} onChange={(e) => (setZone(e.target.value), setLimit(PAGE))}>
              <option value="">{t('Toutes', 'All')}</option>
              {zoneNames.map((z) => (
                <option key={z} value={z}>
                  {z}
                </option>
              ))}
            </select>
          </label>
        )}
        {ai && (
          <label className="flex items-center gap-2 text-sm">
            <span className="text-muted">{t('Plaque', 'Plate')}</span>
            <input
              className="h-8 w-32 rounded-md border border-line bg-bg-2 px-2 text-sm uppercase"
              value={plateInput}
              placeholder="AB-123"
              maxLength={16}
              aria-label={t('Rechercher une plaque', 'Search a plate')}
              onChange={(e) => setPlateInput(e.target.value)}
            />
          </label>
        )}
        <div className="flex items-center gap-2 text-sm">
          <span className="text-muted">{t('Période', 'Period')}</span>
          <Segmented label={t('Période', 'Period')} options={[...PERIODS]} value={hours} onChange={(h) => (setHours(h), setLimit(PAGE))} />
        </div>
        <fieldset className="flex flex-wrap items-center gap-3 text-sm">
          <legend className="sr-only">{t("Type d'événement", "Event type")}</legend>
          <span className="text-muted">{t('Type', 'Type')}</span>
          {KINDS.map(({ kind, needsAi }) => {
            const disabled = needsAi && !ai
            return (
              <label key={kind} className={`flex items-center gap-1.5 ${disabled ? 'opacity-45' : ''}`} title={disabled ? t("Nécessite un fournisseur d'IA (désactivé).", "Needs an AI provider (off).") : undefined}>
                <input
                  type="checkbox"
                  className="size-4 accent-accent"
                  disabled={disabled}
                  checked={kinds.includes(kind)}
                  onChange={(e) => setKinds((k) => (e.target.checked ? [...k, kind] : k.filter((x) => x !== kind)))}
                />
                {EVENT_KIND_LABEL[kind]}
              </label>
            )
          })}
        </fieldset>
        <span className="ml-auto text-xs text-muted tabular">{events.data ? t(`${list.length} événement(s)`, `${list.length} event(s)`) : ''}</span>
      </Card>

      {events.error ? (
        <Card>
          <ErrorState error={events.error} onRetry={events.reload} />
        </Card>
      ) : !events.data ? (
        <LoadingState />
      ) : list.length === 0 ? (
        <Card>
          <EmptyState title={t('Aucun événement pour ces filtres', 'No event for these filters')} />
        </Card>
      ) : listView ? (
        <Card>
          <ul className="divide-y divide-line">
            {items.slice(0, limit).map((i) =>
              i.kind === 'journey' ? (
                <JourneyRow key={`j${i.id}`} id={i.id} events={i.events} name={name} onClick={() => navigate(journeyPlaybackUrl(i.events))} />
              ) : (
                <li key={i.event.id} className="group relative list-none">
                  <EventRow event={i.event} cameraName={name(i.event.camera)} onClick={() => open(i.event.camera, i.event.start_ms)} />
                  {trash(i.event, 'absolute top-1/2 right-3 -translate-y-1/2')}
                </li>
              ),
            )}
          </ul>
        </Card>
      ) : (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 2xl:grid-cols-6">
          {items.slice(0, limit).map((i) =>
            i.kind === 'journey' ? (
              <JourneyCard key={`j${i.id}`} id={i.id} events={i.events} name={name} onClick={() => navigate(journeyPlaybackUrl(i.events))} />
            ) : (
              <div key={i.event.id} className="group relative">
                <EventCard event={i.event} cameraName={name(i.event.camera)} onClick={() => open(i.event.camera, i.event.start_ms)} />
                {trash(i.event, 'absolute top-2 right-2')}
              </div>
            ),
          )}
        </div>
      )}
      {deleteError && <p className="mt-3 rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{deleteError}</p>}
      <ConfirmDialog
        open={toDelete !== null}
        danger
        title={t('Supprimer cet événement ?', 'Delete this event?')}
        message={toDelete ? t(`L’événement de ${formatTime(toDelete.start_ms)} et son image sont effacés pour de bon. La vidéo reste (page Vidéos pour la supprimer).`, `The event of ${formatTime(toDelete.start_ms)} and its picture are erased for good. The video stays (Videos page to delete it).`) : ''}
        confirmLabel={t('Supprimer', 'Delete')}
        onConfirm={() => toDelete && void removeEvent(toDelete)}
        onCancel={() => setToDelete(null)}
      />
      {items.length > limit && (
        <div className="mt-4 flex justify-center">
          <Button onClick={() => setLimit((l) => l + PAGE)}>{t('Afficher plus (', 'Show more (')}{items.length - limit}{t(' restants)', ' left)')}</Button>
        </div>
      )}
    </>
  )
}
