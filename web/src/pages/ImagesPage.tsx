import { ChevronLeft, ChevronRight, Images, Sparkles } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { describeError } from '../api/client'
import { captureUrl, getAi, getCaptureDays, getCapturePictures, replayCaptures } from '../api/endpoints'
import { useAuth, useReadOnly } from '../auth/AuthGate'
import { Button } from '../components/ui/Button'
import { Card, PageHeader } from '../components/ui/Card'
import { Modal } from '../components/ui/Modal'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { useIsDemo, useLive } from '../data/DataContext'
import { useAsync } from '../hooks/useAsync'
import { formatBytes, formatDateTime, formatTime } from '../lib/format'
import { t } from '../lib/i18n'
import { objectLabel } from '../lib/objects'
import { Link } from '../lib/router'
import type { CapturePicture, DetectedObject, ReplayResult } from '../types/api'

/** « Personne 81 %, Voiture 64 % » (objects that do not count marked), or « rien ». */
function describe(found: DetectedObject[], kept: string[]): string {
  if (found.length === 0) return t('rien', 'nothing')
  return found
    .map((o) => `${objectLabel(o.label)} ${Math.round(o.score * 100)} %${kept.includes(o.label) ? '' : t(' (ne compte pas)', ' (does not count)')}`)
    .join(', ')
}

function source(p: CapturePicture): string {
  if (p.line) return t(`Ligne « ${p.line} »`, `Line « ${p.line} »`)
  if (p.event) return t(`Événement ${p.event} · image ${p.picture ?? 1}`, `Event ${p.event} · picture ${p.picture ?? 1}`)
  return t('Essai', 'Test')
}

function Boxes({ objects, className, labels }: { objects: DetectedObject[]; className: string; labels?: boolean }) {
  return (
    <>
      {objects.map((o, i) =>
        o.bbox ? (
          <div
            key={i}
            className={`pointer-events-none absolute rounded-sm border-2 ${className}`}
            style={{ left: `${o.bbox[0] * 100}%`, top: `${o.bbox[1] * 100}%`, width: `${(o.bbox[2] - o.bbox[0]) * 100}%`, height: `${(o.bbox[3] - o.bbox[1]) * 100}%` }}
            aria-hidden
          >
            {labels && (
              <span className="absolute -top-5 left-0 rounded bg-accent px-1 text-[10px] whitespace-nowrap text-white">
                {objectLabel(o.label)} {Math.round(o.score * 100)} %
              </span>
            )}
          </div>
        ) : null,
      )}
    </>
  )
}

/** A kept picture, large, with the AI run on it again on demand. */
function Viewer({ camera, day, pictures, index, onIndex, onClose, aiActive }: { camera: string; day: string; pictures: CapturePicture[]; index: number; onIndex: (i: number) => void; onClose: () => void; aiActive: boolean }) {
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && (!user || user.role === 'admin')
  const p = pictures[index]
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [now, setNow] = useState<ReplayResult['pictures'][number] | null>(null)
  // Another picture: its own analysis.
  useEffect(() => {
    setNow(null)
    setError(null)
  }, [index])
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === 'ArrowLeft' && index > 0) onIndex(index - 1)
      if (e.key === 'ArrowRight' && index < pictures.length - 1) onIndex(index + 1)
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [index, pictures.length, onIndex])
  if (!p) return null
  const analyse = async () => {
    setBusy(true)
    setError(null)
    try {
      const r = await replayCaptures(camera, day, p.file)
      setNow(r.pictures[0] ?? null)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      open
      wide
      title={`${formatDateTime(p.at_ms)} · ${source(p)}`}
      onClose={onClose}
      footer={
        <div className="flex w-full flex-wrap items-center justify-between gap-2">
          <div className="flex gap-2">
            <Button size="sm" icon={<ChevronLeft className="size-4" />} disabledReason={index === 0 ? t('Première image.', 'First picture.') : null} onClick={() => onIndex(index - 1)}>
              {t('Précédente', 'Previous')}
            </Button>
            <Button size="sm" icon={<ChevronRight className="size-4" />} disabledReason={index >= pictures.length - 1 ? t('Dernière image.', 'Last picture.') : null} onClick={() => onIndex(index + 1)}>
              {t('Suivante', 'Next')}
            </Button>
          </div>
          <span className="text-xs text-muted">
            {index + 1} / {pictures.length} · {t('flèches ← → du clavier', 'keyboard arrows ← →')}
          </span>
        </div>
      }
    >
      <div className="space-y-3">
        <div className="relative overflow-hidden rounded-md bg-bg-2">
          <img src={captureUrl(camera, day, p.file)} alt="" className="block w-full" />
          <Boxes objects={p.found} className="border-dashed border-warn" />
          {now && <Boxes objects={now.found} className="border-accent" labels />}
        </div>
        <div className="space-y-1 text-sm">
          <p>
            <span className="text-muted">{t('Trouvé à l’époque : ', 'Found then: ')}</span>
            {describe(p.found, p.kept)}
            <span className="text-xs text-muted">
              {' '}
              · {p.provider} · {p.ms} ms
            </span>
          </p>
          {now && (
            <p className={now.changed ? 'font-medium text-warn' : ''}>
              <span className="text-muted">{t('Maintenant : ', 'Now: ')}</span>
              {describe(now.found, now.kept)}
              <span className="text-xs text-muted">
                {' '}
                · {now.ms} ms · {now.changed ? t('différent', 'different') : t('identique', 'same')}
              </span>
            </p>
          )}
          <p className="text-xs text-muted">{t('Cadres pointillés : trouvés à l’époque ; cadres pleins : trouvés maintenant, avec les réglages actuels de l’IA.', 'Dashed boxes: found then; solid boxes: found now, with the current AI settings.')}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="primary"
            icon={<Sparkles className="size-4" />}
            disabledReason={!aiActive ? t('Reconnaissance d’objets arrêtée (Paramètres → Intelligence artificielle).', 'Object recognition off (Settings → Artificial intelligence).') : !admin ? t('Réservé aux administrateurs.', 'Administrators only.') : busy ? t('Analyse…', 'Analysing…') : null}
            onClick={() => void analyse()}
          >
            {now ? t('Analyser de nouveau', 'Analyse again') : t('Analyser avec l’IA', 'Analyse with the AI')}
          </Button>
          {error && <span className="text-sm text-err">{error}</span>}
        </div>
      </div>
    </Modal>
  )
}

/** « Images »: the pictures kept by `ai.keep_pictures`, opened large and analysed again. */
export function ImagesPage() {
  const demo = useIsDemo()
  const cameras = useLive((s) => s.cameras)
  const name = (id: string) => cameras.find((c) => c.id === id)?.name ?? id
  const days = useAsync(() => (demo ? Promise.resolve([]) : getCaptureDays()), [demo])
  const ai = useAsync(() => (demo ? Promise.resolve(null) : getAi()), [demo])
  const list = useMemo(() => days.data ?? [], [days.data])
  const cameraIds = [...new Set(list.map((d) => d.camera))]
  const [camera, setCamera] = useState<string | null>(null)
  const cam = camera && cameraIds.includes(camera) ? camera : (cameraIds[0] ?? null)
  const camDays = list.filter((d) => d.camera === cam)
  const [day, setDay] = useState<string | null>(null)
  const chosenDay = day && camDays.some((d) => d.day === day) ? day : (camDays[0]?.day ?? null)
  const pictures = useAsync(() => (cam && chosenDay ? getCapturePictures(cam, chosenDay) : Promise.resolve([])), [cam, chosenDay])
  const [open, setOpen] = useState<number | null>(null)
  const [onlyFound, setOnlyFound] = useState(false)
  const all = pictures.data ?? []
  const shown = all.map((p, i) => ({ p, i })).filter(({ p }) => !onlyFound || p.kept.length > 0)

  const header = <PageHeader title={t('Images', 'Pictures')} description={t('Images analysées par l’IA et ce qu’elle y a trouvé. Cliquez sur une image pour l’agrandir et la faire analyser de nouveau avec les réglages actuels.', 'Pictures analysed by the AI and what it found. Click a picture to enlarge it and have it analysed again with the current settings.')} />
  if (demo) {
    return (
      <>
        {header}
        <Card>
          <EmptyState title={t('Non disponible en mode démo', 'Not available in demo mode')} icon={<Images className="size-7" />} />
        </Card>
      </>
    )
  }
  return (
    <>
      {header}
      {days.error ? (
        <ErrorState error={days.error} onRetry={days.reload} />
      ) : days.loading && !days.data ? (
        <LoadingState />
      ) : list.length === 0 ? (
        <Card>
          <EmptyState title={t('Aucune image gardée', 'No kept picture')} icon={<Images className="size-7" />}>
            <p className="max-w-md text-sm text-muted">
              {t('Cochez « Garder les images analysées » dans ', 'Tick « Keep the analysed pictures » in ')}
              <Link to="/settings?section=ai" className="text-accent hover:underline">
                {t('Paramètres → Intelligence artificielle', 'Settings → Artificial intelligence')}
              </Link>
              {t(', puis attendez quelques événements.', ', then wait for a few events.')}
            </p>
          </EmptyState>
        </Card>
      ) : (
        <div className="space-y-4">
          <div className="flex flex-wrap items-end gap-3">
            <label className="text-sm">
              {t('Caméra', 'Camera')}
              <select className="mt-1 block h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={cam ?? ''} onChange={(e) => setCamera(e.target.value)}>
                {cameraIds.map((c) => (
                  <option key={c} value={c}>
                    {name(c)}
                  </option>
                ))}
              </select>
            </label>
            <label className="text-sm">
              {t('Journée (UTC)', 'Day (UTC)')}
              <select className="mt-1 block h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={chosenDay ?? ''} onChange={(e) => setDay(e.target.value)}>
                {camDays.map((d) => (
                  <option key={d.day} value={d.day}>
                    {d.day} · {d.count} {t('images', 'pictures')} · {formatBytes(d.bytes)}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex h-9 items-center gap-2 text-sm">
              <input type="checkbox" className="size-4 accent-accent" checked={onlyFound} onChange={(e) => setOnlyFound(e.target.checked)} />
              {t('Seulement avec un objet', 'Only with an object')}
            </label>
          </div>
          {pictures.error ? (
            <ErrorState error={pictures.error} onRetry={pictures.reload} />
          ) : pictures.loading && !pictures.data ? (
            <LoadingState />
          ) : (
            <ul className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
              {shown.map(({ p, i }) => (
                <li key={p.file}>
                  <button type="button" className="card group block w-full overflow-hidden text-left" onClick={() => setOpen(i)}>
                    <div className="relative aspect-video bg-bg-2">
                      <img src={captureUrl(cam ?? '', chosenDay ?? '', p.file)} alt="" loading="lazy" decoding="async" className="h-full w-full object-cover transition-opacity group-hover:opacity-90" />
                      <Boxes objects={p.found} className="border-accent" />
                    </div>
                    <div className="space-y-0.5 px-2 py-1.5 text-xs">
                      <p className="text-muted">
                        {formatTime(p.at_ms, true)} · {source(p)}
                      </p>
                      <p className={p.kept.length > 0 ? 'font-medium' : 'text-muted'}>{describe(p.found, p.kept)}</p>
                    </div>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {shown.length === 0 && pictures.data && <p className="text-sm text-muted">{t('Aucune image avec un objet sur cette journée.', 'No picture with an object on this day.')}</p>}
        </div>
      )}
      {open !== null && cam && chosenDay && (
        <Viewer camera={cam} day={chosenDay} pictures={all} index={open} onIndex={setOpen} onClose={() => setOpen(null)} aiActive={ai.data?.active != null} />
      )}
    </>
  )
}
