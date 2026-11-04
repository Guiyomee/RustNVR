import { BarChart3, LayoutGrid, List } from 'lucide-react'
import { useMemo, useState } from 'react'
import { UnavailableError } from '../api/client'
import { ActivityStats } from '../components/events/ActivityStats'
import { EVENT_KIND_LABEL, EventCard, EventRow } from '../components/events/EventCard'
import { Button } from '../components/ui/Button'
import { Card, PageHeader } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState, Unavailable } from '../components/ui/States'
import { Segmented } from '../components/ui/Tabs'
import { useCapability, useIsDemo, useLive } from '../data/DataContext'
import { useEvents } from '../hooks/useEvents'
import { useNow, useStoredFlag } from '../hooks/useUi'
import { navigate } from '../lib/router'
import type { EventKind } from '../types/api'
import { t } from '../lib/i18n'

const PERIODS = [
  { value: 1, label: '1 h' },
  { value: 6, label: '6 h' },
  { value: 24, label: '24 h' },
  { value: 168, label: t('7 j', '7 d') },
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
  const query = useMemo(() => ({ camera: camera || null, fromMs: now - hours * 3_600_000, toMs: now, kinds }), [camera, hours, kinds, now])
  const events = useEvents(query)
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
          <select className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm" value={camera} onChange={(e) => (setCamera(e.target.value), setLimit(PAGE))}>
            <option value="">{t('Toutes', 'All')}</option>
            {cameras.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
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
            {list.slice(0, limit).map((e) => (
              <EventRow key={e.id} event={e} cameraName={names.get(e.camera) ?? e.camera} onClick={() => open(e.camera, e.start_ms)} />
            ))}
          </ul>
        </Card>
      ) : (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 2xl:grid-cols-6">
          {list.slice(0, limit).map((e) => (
            <EventCard key={e.id} event={e} cameraName={names.get(e.camera) ?? e.camera} onClick={() => open(e.camera, e.start_ms)} />
          ))}
        </div>
      )}
      {list.length > limit && (
        <div className="mt-4 flex justify-center">
          <Button onClick={() => setLimit((l) => l + PAGE)}>{t('Afficher plus (', 'Show more (')}{list.length - limit}{t(' restants)', ' left)')}</Button>
        </div>
      )}
    </>
  )
}
