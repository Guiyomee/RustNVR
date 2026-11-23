import { ArrowRight, Plus, Save, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { getJourneySettings, setJourneySettings } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useCapability, useData, useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { t } from '../../lib/i18n'
import type { JourneyLink } from '../../types/api'
import { Button } from '../ui/Button'
import { ErrorState, LoadingState } from '../ui/States'

const MAX_LINKS = 64
const MAX_DELAY_S = 600
const DEFAULT_DELAY_S = 30

const field = 'h-8 rounded-md border border-line bg-bg-2 px-2 text-sm disabled:opacity-60'

/** What is wrong with the arrows, or null (the server checks it again). */
function problem(links: JourneyLink[], name: (id: string) => string): string | null {
  for (const [i, l] of links.entries()) {
    if (!l.from || !l.to) return t('Choisissez les deux caméras de chaque flèche.', 'Choose both cameras of each arrow.')
    if (l.from === l.to) return t(`${name(l.from)} vers elle-même : choisissez une autre caméra.`, `${name(l.from)} to itself: choose another camera.`)
    if (!Number.isInteger(l.max_delay_s) || l.max_delay_s < 1 || l.max_delay_s > MAX_DELAY_S) return t(`Délai de 1 à ${MAX_DELAY_S} s.`, `Delay from 1 to ${MAX_DELAY_S} s.`)
    const twin = links.slice(0, i).some((o) => (o.from === l.from && o.to === l.to) || (o.from === l.to && o.to === l.from))
    if (twin) return t(`La flèche ${name(l.from)} → ${name(l.to)} est en double.`, `The arrow ${name(l.from)} → ${name(l.to)} is there twice.`)
  }
  return null
}

/** The arrows between cameras for the journeys (`/journeys/settings`). */
export function JourneySettingsForm() {
  const demo = useIsDemo()
  const available = useCapability('journeys') === true
  const ai = useCapability('ai') === true
  const readOnly = useReadOnly()
  const cameras = useLive((s) => s.cameras)
  const { source } = useData()
  const loaded = useAsync(() => (demo || !available ? Promise.resolve(null) : getJourneySettings()), [demo, available])
  const [saved, setSaved] = useState<JourneyLink[] | null>(null)
  const [links, setLinks] = useState<JourneyLink[]>([])
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    if (loaded.data) {
      setSaved(loaded.data.links)
      setLinks(loaded.data.links)
    }
  }, [loaded.data])

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Indisponible en mode démo.', 'Not available in demo mode.')}</p>
  if (!available) return <p className="px-5 py-4 text-sm text-muted">{t("Ce serveur n'a pas de base de données : pas de trajets.", 'This server has no database: no journeys.')}</p>
  if (loaded.error) return <ErrorState error={loaded.error} onRetry={loaded.reload} />
  if (!saved) return <LoadingState />

  const name = (id: string) => cameras.find((c) => c.id === id)?.name ?? id
  const edit = (i: number, change: Partial<JourneyLink>) => setLinks((ls) => ls.map((l, j) => (j === i ? { ...l, ...change } : l)))
  const dirty = JSON.stringify(links) !== JSON.stringify(saved)
  const wrong = problem(links, name)
  const locked = readOnly !== null || busy

  const add = () => {
    // The first two cameras not joined yet, as a start.
    const [a, b] = cameras
    setLinks((ls) => [...ls, { from: a?.id ?? '', to: b?.id ?? '', max_delay_s: DEFAULT_DELAY_S, both_ways: true }])
  }
  const save = async () => {
    setBusy(true)
    try {
      const out = await setJourneySettings({ links })
      setSaved(out.links)
      setLinks(out.links)
      source.store.notify('info', t('Trajets enregistrés', 'Journeys saved'))
    } catch (err) {
      source.store.notify('error', t(`Trajets : ${describeError(err)}`, `Journeys: ${describeError(err)}`))
    } finally {
      setBusy(false)
    }
  }

  const cameraSelect = (value: string, onChange: (id: string) => void, label: string) => (
    <select className={`${field} min-w-0 flex-1`} aria-label={label} value={value} disabled={locked} onChange={(e) => onChange(e.target.value)}>
      {!cameras.some((c) => c.id === value) && <option value={value}>{value || '—'}</option>}
      {cameras.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </select>
  )

  return (
    <div className="space-y-3 px-5 py-4">
      {!ai && (
        <p className="rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-xs text-warn">
          {t("Aucun fournisseur d'IA n'est activé (Paramètres → Intelligence artificielle) : sans objet reconnu, aucun trajet ne sera formé.", 'No AI provider is on (Settings → Artificial intelligence): without recognized objects, no journey will be made.')}
        </p>
      )}
      {links.length === 0 ? (
        <p className="text-sm text-muted">
          {t('Aucune flèche. Exemple : Sonnette → Garage, Sonnette → Entrée, Garage → Entrée, Entrée → Salon ; les parcours qui les suivent (entiers ou en partie) sont reconnus.', 'No arrow. Example: Doorbell → Garage, Doorbell → Entrance, Garage → Entrance, Entrance → Living room; the paths that follow them (whole or in part) are recognized.')}
        </p>
      ) : (
        <ul className="space-y-2">
          {links.map((l, i) => (
            <li key={i} className="flex flex-wrap items-center gap-2 rounded-md border border-line p-2">
              <div className="flex min-w-[260px] flex-1 items-center gap-2">
                {cameraSelect(l.from, (from) => edit(i, { from }), t('De', 'From'))}
                <ArrowRight className="size-4 shrink-0 text-muted" aria-hidden />
                {cameraSelect(l.to, (to) => edit(i, { to }), t('Vers', 'To'))}
              </div>
              <label className="flex items-center gap-1.5 text-xs text-muted">
                {t('Délai max', 'Max delay')}
                <input
                  type="number"
                  min={1}
                  max={MAX_DELAY_S}
                  className={`${field} w-20 tabular`}
                  value={l.max_delay_s}
                  disabled={locked}
                  onChange={(e) => edit(i, { max_delay_s: Math.round(Number(e.target.value)) })}
                />
                s
              </label>
              <label className="flex items-center gap-1.5 text-xs" title={t('Aussi pour quelqu’un qui part (trajet noté « départ »).', 'Also for someone leaving (journey marked « departure »).')}>
                <input type="checkbox" className="size-4 accent-accent" checked={l.both_ways} disabled={locked} onChange={(e) => edit(i, { both_ways: e.target.checked })} />
                {t('Aussi dans l’autre sens', 'Also the other way')}
              </label>
              <Button
                size="sm"
                variant="ghost"
                icon={<Trash2 className="size-3.5" />}
                aria-label={t('Retirer la flèche', 'Remove the arrow')}
                disabledReason={readOnly ?? (busy ? t('Enregistrement…', 'Saving…') : null)}
                onClick={() => setLinks((ls) => ls.filter((_, j) => j !== i))}
              />
            </li>
          ))}
        </ul>
      )}
      <p className="text-xs text-muted">
        {t('Délai max : le temps le plus long entre la fin de l’événement sur la première caméra et le début de celui sur la suivante (on peut s’arrêter en chemin : comptez large).', 'Max delay: the longest time between the end of the event on the first camera and the start of the one on the next (one may stop on the way: allow plenty).')}
      </p>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Button
          size="sm"
          icon={<Plus className="size-3.5" />}
          disabledReason={
            readOnly ??
            (cameras.length < 2 ? t('Il faut au moins deux caméras.', 'At least two cameras are needed.') : links.length >= MAX_LINKS ? t(`${MAX_LINKS} flèches au plus.`, `${MAX_LINKS} arrows at most.`) : busy ? t('Enregistrement…', 'Saving…') : null)
          }
          onClick={add}
        >
          {t('Ajouter une flèche', 'Add an arrow')}
        </Button>
        <Button
          variant="primary"
          icon={<Save className="size-4" />}
          disabledReason={readOnly ?? wrong ?? (!dirty ? t('Aucun changement.', 'No change.') : busy ? t('Enregistrement…', 'Saving…') : null)}
          onClick={() => void save()}
        >
          {t('Enregistrer', 'Save')}
        </Button>
      </div>
    </div>
  )
}
