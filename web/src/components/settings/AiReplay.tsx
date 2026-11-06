import { Play, Trash2 } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { captureUrl, deleteCaptureDay, getCaptureDays, replayCaptures } from '../../api/endpoints'
import { useAuth, useReadOnly } from '../../auth/AuthGate'
import { useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatBytes, formatDateTime } from '../../lib/format'
import { t } from '../../lib/i18n'
import { objectLabel } from '../../lib/objects'
import type { DetectedObject, ReplayResult } from '../../types/api'
import { Button } from '../ui/Button'

/** « Personne 81 %, Voiture 64 % », or « rien ». */
function describe(found: DetectedObject[], kept: string[]): string {
  if (found.length === 0) return t('rien', 'nothing')
  return found
    .map((o) => `${objectLabel(o.label)} ${Math.round(o.score * 100)} %${kept.includes(o.label) ? '' : t(' (ne compte pas)', ' (does not count)')}`)
    .join(', ')
}

function Boxes({ objects, className }: { objects: DetectedObject[]; className: string }) {
  return (
    <>
      {objects.map((o, i) =>
        o.bbox ? (
          <div
            key={i}
            className={`pointer-events-none absolute rounded-sm border-2 ${className}`}
            style={{ left: `${o.bbox[0] * 100}%`, top: `${o.bbox[1] * 100}%`, width: `${(o.bbox[2] - o.bbox[0]) * 100}%`, height: `${(o.bbox[3] - o.bbox[1]) * 100}%` }}
            aria-hidden
          />
        ) : null,
      )}
    </>
  )
}

/**
 * Replays the object recognition on the pictures kept by `ai.keep_pictures`
 * and shows, picture by picture, what it finds now against what it found.
 */
export function AiReplay({ active }: { active: boolean }) {
  const cameras = useLive((s) => s.cameras)
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && (!user || user.role === 'admin')
  const days = useAsync(() => getCaptureDays(), [])
  const [chosen, setChosen] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<{ camera: string; day: string; r: ReplayResult } | null>(null)
  const [onlyChanged, setOnlyChanged] = useState(true)
  const name = (id: string) => cameras.find((c) => c.id === id)?.name ?? id
  const list = days.data ?? []
  const selected = list.find((d) => `${d.camera}/${d.day}` === chosen) ?? list[0]

  const run = async () => {
    if (!selected) return
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      const r = await replayCaptures(selected.camera, selected.day)
      setResult({ camera: selected.camera, day: selected.day, r })
      setOnlyChanged(r.changed > 0)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    if (!selected) return
    setError(null)
    try {
      await deleteCaptureDay(selected.camera, selected.day)
      setResult(null)
      setChosen('')
      days.reload()
    } catch (e) {
      setError(describeError(e))
    }
  }
  const blocked = !admin ? t('Réservé aux administrateurs.', 'Administrators only.') : busy ? t('Rejeu en cours…', 'Replaying…') : null
  const shown = result ? result.r.pictures.filter((p) => !onlyChanged || p.changed) : []

  return (
    <div className="space-y-3 rounded-md border border-line p-4">
      <h3 className="text-sm font-medium">{t('Rejouer l’IA sur les images gardées', 'Replay the AI on the kept pictures')}</h3>
      <p className="text-xs text-muted">
        {t(
          'Analyse de nouveau les images d’une journée avec les réglages actuels (modèle, score minimal, objets, zones, lignes), et montre ce qui change : utile pour essayer un autre modèle ou un autre seuil sur de vraies images. 200 images au plus par rejeu.',
          'Analyses the pictures of a day again with the current settings (model, minimum score, objects, zones, lines), and shows what changes: useful to try another model or threshold on real pictures. 200 pictures at most per replay.',
        )}
      </p>
      {days.error ? (
        <p className="text-sm text-err">{describeError(days.error)}</p>
      ) : list.length === 0 ? (
        <p className="text-sm text-muted">{days.loading ? t('Chargement…', 'Loading…') : t('Aucune image gardée : cochez « Garder les images analysées » ci-dessus, puis attendez quelques événements.', 'No kept picture: tick « Keep the analysed pictures » above, then wait for a few events.')}</p>
      ) : (
        <div className="flex flex-wrap items-end gap-2">
          <label className="text-sm">
            {t('Journée', 'Day')}
            <select className="mt-1 block h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={selected ? `${selected.camera}/${selected.day}` : ''} onChange={(e) => setChosen(e.target.value)}>
              {list.map((d) => (
                <option key={`${d.camera}/${d.day}`} value={`${d.camera}/${d.day}`}>
                  {name(d.camera)} · {d.day} · {d.count} {t('images', 'pictures')} · {formatBytes(d.bytes)}
                </option>
              ))}
            </select>
          </label>
          <Button variant="primary" icon={<Play className="size-4" />} disabledReason={!active ? t('Reconnaissance d’objets arrêtée.', 'Object recognition off.') : blocked} onClick={() => void run()}>
            {t('Rejouer l’IA', 'Replay the AI')}
          </Button>
          <Button icon={<Trash2 className="size-4" />} disabledReason={blocked} onClick={() => void remove()}>
            {t('Supprimer cette journée', 'Delete this day')}
          </Button>
        </div>
      )}
      {busy && <p className="text-xs text-muted">{t('Analyse en cours : environ 0,05 s par image avec le modèle intégré, 0,5 s avec CodeProject.AI sur processeur.', 'Analysing: about 0.05 s per picture with the built-in model, 0.5 s with CodeProject.AI on a processor.')}</p>}
      {error && <p className="text-sm text-err">{error}</p>}
      {result && (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <span>
              {t(`${result.r.pictures.length} images rejouées (${result.r.provider}) : `, `${result.r.pictures.length} pictures replayed (${result.r.provider}): `)}
              <span className={result.r.changed > 0 ? 'font-semibold text-warn' : 'text-ok'}>
                {result.r.changed > 0 ? t(`${result.r.changed} différente(s)`, `${result.r.changed} different`) : t('aucune différence', 'no difference')}
              </span>
              {result.r.skipped > 0 && t(` · ${result.r.skipped} non rejouées (limite de 200)`, ` · ${result.r.skipped} not replayed (limit of 200)`)}
            </span>
            <label className="flex items-center gap-2 text-xs">
              <input type="checkbox" className="size-4 accent-accent" checked={onlyChanged} onChange={(e) => setOnlyChanged(e.target.checked)} />
              {t('Seulement les différences', 'Only the differences')}
            </label>
          </div>
          <p className="text-xs text-muted">{t('Cadres pointillés : trouvés à l’époque ; cadres pleins : trouvés maintenant.', 'Dashed boxes: found then; solid boxes: found now.')}</p>
          <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {shown.map((p) => {
              const b = p.before
              const source = b.line ? t(`Ligne « ${b.line} »`, `Line « ${b.line} »`) : b.event ? t(`Événement ${b.event} · image ${b.picture ?? 1}`, `Event ${b.event} · picture ${b.picture ?? 1}`) : t('Essai', 'Test')
              return (
                <li key={b.file} className={`overflow-hidden rounded-md border ${p.changed ? 'border-warn' : 'border-line'}`}>
                  <div className="relative aspect-video bg-bg-2">
                    <img src={captureUrl(result.camera, result.day, b.file)} alt="" loading="lazy" className="h-full w-full object-cover" />
                    <Boxes objects={b.found} className="border-dashed border-warn" />
                    <Boxes objects={p.found} className="border-accent" />
                  </div>
                  <div className="space-y-0.5 p-2 text-xs">
                    <p className="text-muted">
                      {formatDateTime(b.at_ms)} · {source}
                    </p>
                    <p>
                      <span className="text-muted">{t('Avant : ', 'Then: ')}</span>
                      {describe(b.found, b.kept)}
                    </p>
                    <p className={p.changed ? 'font-medium text-warn' : ''}>
                      <span className="text-muted">{t('Maintenant : ', 'Now: ')}</span>
                      {describe(p.found, p.kept)}
                    </p>
                  </div>
                </li>
              )
            })}
          </ul>
          {shown.length === 0 && <p className="text-sm text-muted">{t('Aucune différence sur cette journée.', 'No difference on this day.')}</p>}
        </div>
      )}
    </div>
  )
}
