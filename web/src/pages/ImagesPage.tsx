import { ChevronLeft, ChevronRight, Images, Sparkles, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { describeError } from '../api/client'
import { captureUrl, deleteCapture, getAi, getCaptureDays, getCapturePictures, replayCaptures } from '../api/endpoints'
import { useAuth, useReadOnly } from '../auth/AuthGate'
import { Button } from '../components/ui/Button'
import { Card, PageHeader } from '../components/ui/Card'
import { ConfirmDialog, Modal } from '../components/ui/Modal'
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
function Viewer({ camera, day, pictures, index, onIndex, onClose, onDeleted, aiActive }: { camera: string; day: string; pictures: CapturePicture[]; index: number; onIndex: (i: number) => void; onClose: () => void; onDeleted: () => void; aiActive: boolean }) {
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && (!user || user.role === 'admin')
  const p = pictures[index]
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [now, setNow] = useState<ReplayResult['pictures'][number] | null>(null)
  const [confirming, setConfirming] = useState(false)
  const [deleting, setDeleting] = useState(false)
  // Another picture: its own analysis.
  useEffect(() => {
    setNow(null)
    setError(null)
  }, [index])
  const analyseReason = !aiActive ? t('Reconnaissance d’objets arrêtée (Paramètres → Intelligence artificielle).', 'Object recognition off (Settings → Artificial intelligence).') : !admin ? t('Réservé aux administrateurs.', 'Administrators only.') : busy ? t('Analyse…', 'Analysing…') : null
  const deleteReason = !admin ? t('Réservé aux administrateurs.', 'Administrators only.') : deleting ? t('Suppression…', 'Deleting…') : null
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (confirming || e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return
      if (e.key === 'ArrowLeft' && index > 0) onIndex(index - 1)
      else if (e.key === 'ArrowRight' && index < pictures.length - 1) onIndex(index + 1)
      else if ((e.key === 'Delete' || e.key === 'Backspace') && !deleteReason) {
        e.preventDefault()
        setConfirming(true)
      } else if ((e.key === 'a' || e.key === 'A') && !analyseReason) void analyseRef.current()
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [index, pictures.length, onIndex, confirming, deleteReason, analyseReason])
  const analyseRef = useRef(async () => {})
  if (!p) return null
  const analyse = async () => {
    if (busy) return
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
  analyseRef.current = analyse
  const remove = async () => {
    setConfirming(false)
    setDeleting(true)
    setError(null)
    try {
      await deleteCapture(camera, day, p.file)
      onDeleted()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setDeleting(false)
    }
  }
  return (
    <Modal
      open
      wide
      title={`${formatDateTime(p.at_ms)} · ${source(p)}`}
      onClose={onClose}
      footer={
        <div className="flex w-full flex-wrap items-center gap-2">
          <Button size="sm" icon={<ChevronLeft className="size-4" />} disabledReason={index === 0 ? t('Première image.', 'First picture.') : null} onClick={() => onIndex(index - 1)}>
            {t('Précédente', 'Previous')}
          </Button>
          <span className="tabular px-1 text-xs text-muted">
            {index + 1} / {pictures.length}
          </span>
          <Button size="sm" icon={<ChevronRight className="size-4" />} disabledReason={index >= pictures.length - 1 ? t('Dernière image.', 'Last picture.') : null} onClick={() => onIndex(index + 1)}>
            {t('Suivante', 'Next')}
          </Button>
          <span className="flex-1" />
          <Button size="sm" variant="primary" icon={<Sparkles className="size-4" />} disabledReason={analyseReason} onClick={() => void analyse()}>
            {now ? t('Analyser de nouveau', 'Analyse again') : t('Analyser avec l’IA', 'Analyse with the AI')}
          </Button>
          <Button size="sm" variant="danger" icon={<Trash2 className="size-4" />} disabledReason={deleteReason} onClick={() => setConfirming(true)}>
            {t('Supprimer', 'Delete')}
          </Button>
        </div>
      }
    >
      <div className="space-y-2">
        <div className="relative mx-auto w-fit max-w-full overflow-hidden rounded-md bg-black">
          <img src={captureUrl(camera, day, p.file)} alt="" className="block max-h-[58vh] w-auto max-w-full" />
          <Boxes objects={p.found} className="border-dashed border-warn" />
          {now && <Boxes objects={now.found} className="border-accent" labels />}
        </div>
        <div className="space-y-0.5 text-sm">
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
          {error && <p className="text-err">{error}</p>}
          <p className="text-xs text-muted">
            {t('Pointillés : trouvé à l’époque ; trait plein : trouvé maintenant. Clavier : ← → images, A analyser, Suppr supprimer.', 'Dashed: found then; solid: found now. Keyboard: ← → pictures, A analyse, Delete delete.')}
          </p>
        </div>
      </div>
      <ConfirmDialog
        open={confirming}
        danger
        title={t('Supprimer cette image ?', 'Delete this picture?')}
        message={t('L’image et ce que l’IA y a trouvé sont effacés pour de bon. L’événement et sa vidéo restent.', 'The picture and what the AI found in it are erased for good. The event and its video stay.')}
        confirmLabel={t('Supprimer', 'Delete')}
        onConfirm={() => void remove()}
        onCancel={() => setConfirming(false)}
      />
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
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && (!user || user.role === 'admin')
  const [toDelete, setToDelete] = useState<CapturePicture | null>(null)
  const [gridError, setGridError] = useState<string | null>(null)
  const removeTile = async (pic: CapturePicture) => {
    setToDelete(null)
    setGridError(null)
    try {
      await deleteCapture(cam ?? '', chosenDay ?? '', pic.file)
      pictures.reload()
      days.reload()
    } catch (e) {
      setGridError(describeError(e))
    }
  }
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
                <li key={p.file} className="group relative">
                  {admin && (
                    <button
                      type="button"
                      aria-label={t(`Supprimer l’image de ${formatTime(p.at_ms, true)}`, `Delete the picture of ${formatTime(p.at_ms, true)}`)}
                      title={t('Supprimer cette image', 'Delete this picture')}
                      onClick={() => setToDelete(p)}
                      className="absolute top-2 right-2 z-10 flex size-8 items-center justify-center rounded-md bg-black/60 text-white opacity-0 transition-opacity group-hover:opacity-100 hover:bg-err focus-visible:opacity-100"
                    >
                      <Trash2 className="size-4" aria-hidden />
                    </button>
                  )}
                  <button type="button" className="card block w-full overflow-hidden text-left" onClick={() => setOpen(i)}>
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
          {gridError && <p className="text-sm text-err">{gridError}</p>}
          {shown.length === 0 && pictures.data && <p className="text-sm text-muted">{t('Aucune image avec un objet sur cette journée.', 'No picture with an object on this day.')}</p>}
        </div>
      )}
      <ConfirmDialog
        open={toDelete !== null}
        danger
        title={t('Supprimer cette image ?', 'Delete this picture?')}
        message={t('L’image et ce que l’IA y a trouvé sont effacés pour de bon. L’événement et sa vidéo restent.', 'The picture and what the AI found in it are erased for good. The event and its video stay.')}
        confirmLabel={t('Supprimer', 'Delete')}
        onConfirm={() => toDelete && void removeTile(toDelete)}
        onCancel={() => setToDelete(null)}
      />
      {open !== null && cam && chosenDay && (
        <Viewer
          camera={cam}
          day={chosenDay}
          pictures={all}
          index={open}
          onIndex={setOpen}
          onClose={() => setOpen(null)}
          onDeleted={() => {
            // The next picture takes its place; none left: the viewer closes.
            setOpen(all.length <= 1 ? null : Math.min(open, all.length - 2))
            pictures.reload()
            days.reload()
          }}
          aiActive={ai.data?.active != null}
        />
      )}
    </>
  )
}
