import { CircleAlert, Clock, Download, Film, Loader2, Play, RotateCcw, Square, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { deleteTimelapse, getTimelapses, makeTimelapse, timelapseUrl } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useData, useIsDemo } from '../../data/DataContext'
import { useAsync, useInterval } from '../../hooks/useAsync'
import { useNow } from '../../hooks/useUi'
import { formatBytes, fromDateInput, formatDate, startOfDay, toDateInput } from '../../lib/format'
import { t } from '../../lib/i18n'
import type { Camera, Timelapse } from '../../types/api'
import { Button } from '../ui/Button'
import { Card, CardHeader } from '../ui/Card'
import { ConfirmDialog } from '../ui/Modal'
import { EmptyState, ErrorState, LoadingState } from '../ui/States'

const DAY = 86_400_000
/** How often the list is read again while a time-lapse is on its way. */
const REFRESH_MS = 3_000

const ready = (i: Timelapse) => (i.state ?? 'ready') === 'ready'

/** The daily time-lapses of a camera: watched, downloaded, made again. */
export function TimelapseCard({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const readOnly = useReadOnly()
  const { source } = useData()
  const now = useNow(60_000)
  const list = useAsync(() => (demo ? Promise.resolve([]) : getTimelapses(camera.id)), [demo, camera.id])
  const [day, setDay] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // The day asked here: shown in the player once it is ready.
  const [asked, setAsked] = useState<string | null>(null)
  const yesterday = toDateInput(startOfDay(now) - DAY / 2)
  const [makeDay, setMakeDay] = useState(yesterday)
  const items = list.data ?? []
  const videos = items.filter(ready)
  const pending = items.some((i) => !ready(i) && i.state !== 'failed')
  useInterval(list.reload, pending ? REFRESH_MS : null)
  useEffect(() => {
    if (asked && videos.some((v) => v.day === asked)) {
      setDay(asked)
      setAsked(null)
    }
  }, [asked, videos])
  const shown = day && videos.some((i) => i.day === day) ? day : (videos[0]?.day ?? null)
  const label = (d: string) => {
    const ms = fromDateInput(d)
    return ms === null ? d : formatDate(ms)
  }

  const make = async (d: string) => {
    setBusy(true)
    try {
      await makeTimelapse(camera.id, d)
      setAsked(d)
      list.reload()
    } catch (err) {
      source.store.notify('error', `${camera.name} : ${describeError(err)}`, camera.id)
    } finally {
      setBusy(false)
    }
  }
  // Stop (on its way) or remove (a video, a failure): the same call.
  const [removing, setRemoving] = useState<string | null>(null)
  const [toRemove, setToRemove] = useState<string | null>(null)
  const remove = async (d: string) => {
    setRemoving(d)
    try {
      await deleteTimelapse(camera.id, d)
      if (asked === d) setAsked(null)
      list.reload()
    } catch (err) {
      source.store.notify('error', `${camera.name} : ${describeError(err)}`, camera.id)
    } finally {
      setRemoving(null)
    }
  }
  const removeReason = (d: string) =>
    demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : readOnly ?? (removing === d ? t('En cours…', 'In progress…') : null)
  const onItsWay = (d: string) => items.some((i) => i.day === d && (i.state === 'waiting' || i.state === 'making'))
  const makeReason = (d: string) =>
    demo
      ? t('Indisponible en mode démo.', 'Not available in demo mode.')
      : readOnly ?? (busy ? t('Envoi…', 'Sending…') : onItsWay(d) ? t('Ce jour est déjà en préparation.', 'This day is already being made.') : null)

  return (
    <Card>
      <CardHeader
        title={t('Time-lapse quotidien', 'Daily time-lapse')}
        subtitle={t('Une vidéo par jour, gardée 30 jours. À activer dans l\'onglet Détection.', 'One video per day, kept 30 days. Turned on in the Detection tab.')}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <input
              type="date"
              aria-label={t('Jour à préparer', 'Day to make')}
              className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm"
              value={makeDay}
              max={yesterday}
              onChange={(e) => e.target.value && setMakeDay(e.target.value)}
            />
            <Button size="sm" icon={<Film className="size-3.5" />} disabledReason={makeReason(makeDay)} onClick={() => void make(makeDay)}>
              {t('Préparer', 'Make')}
            </Button>
          </div>
        }
      />
      {list.error ? (
        <ErrorState error={list.error} onRetry={list.reload} />
      ) : !list.data ? (
        <LoadingState />
      ) : items.length === 0 ? (
        <EmptyState title={t('Aucun time-lapse', 'No time-lapse')}>
          {t('Cochez « Time-lapse quotidien » dans l\'onglet Détection : celui de la veille est préparé chaque nuit. Ou choisissez un jour et « Préparer ».', 'Tick « Daily time-lapse » in the Detection tab: the one of the day before is made every night. Or pick a day and « Make ».')}
        </EmptyState>
      ) : (
        <div className="grid gap-4 p-4 lg:grid-cols-[minmax(0,1fr)_260px]">
          {shown ? (
            <video key={shown} src={timelapseUrl(camera.id, shown)} controls preload="metadata" className="aspect-video w-full rounded-md bg-black" />
          ) : (
            <div className="flex aspect-video w-full items-center justify-center rounded-md bg-black text-sm text-muted">
              {t('La vidéo s’affichera ici une fois prête.', 'The video will show here once ready.')}
            </div>
          )}
          <ul className="max-h-[360px] divide-y divide-line overflow-y-auto rounded-md border border-line">
            {items.map((i) => (
              <li key={i.day} className={`flex items-center gap-2 px-3 py-2 text-sm ${i.day === shown ? 'bg-line/40' : ''}`}>
                {ready(i) ? (
                  <>
                    <button type="button" className="min-w-0 flex-1 text-left hover:text-accent" onClick={() => setDay(i.day)}>
                      {label(i.day)}
                      <span className="block text-xs text-muted">{formatBytes(i.bytes)}</span>
                    </button>
                    <button
                      type="button"
                      onClick={() => setDay(i.day)}
                      className="inline-flex size-8 items-center justify-center rounded-md border border-line hover:bg-line/60"
                      title={t('Lire', 'Play')}
                      aria-label={t(`Lire le time-lapse du ${label(i.day)}`, `Play the time-lapse of ${label(i.day)}`)}
                    >
                      <Play className="size-3.5" aria-hidden />
                    </button>
                    <a
                      href={timelapseUrl(camera.id, i.day)}
                      download={`${camera.id}-${i.day}.mp4`}
                      className="inline-flex size-8 items-center justify-center rounded-md border border-line hover:bg-line/60"
                      title={t('Télécharger', 'Download')}
                      aria-label={t(`Télécharger le time-lapse du ${label(i.day)}`, `Download the time-lapse of ${label(i.day)}`)}
                    >
                      <Download className="size-3.5" aria-hidden />
                    </a>
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<Trash2 className="size-3.5" />}
                      aria-label={t(`Supprimer le time-lapse du ${label(i.day)}`, `Delete the time-lapse of ${label(i.day)}`)}
                      disabledReason={removeReason(i.day)}
                      onClick={() => setToRemove(i.day)}
                    />
                  </>
                ) : i.state === 'failed' ? (
                  <>
                    <div className="min-w-0 flex-1">
                      {label(i.day)}
                      <span className="flex items-start gap-1 text-xs text-err">
                        <CircleAlert className="mt-px size-3.5 shrink-0" aria-hidden />
                        <span className="break-words">{i.error ?? t('Échec', 'Failed')}</span>
                      </span>
                    </div>
                    <Button size="sm" variant="ghost" icon={<RotateCcw className="size-3.5" />} aria-label={t('Réessayer', 'Try again')} disabledReason={makeReason(i.day)} onClick={() => void make(i.day)} />
                    <Button size="sm" variant="ghost" icon={<Trash2 className="size-3.5" />} aria-label={t('Retirer de la liste', 'Remove from the list')} disabledReason={removeReason(i.day)} onClick={() => void remove(i.day)} />
                  </>
                ) : (
                  <div className="min-w-0 flex-1" aria-live="polite">
                    {label(i.day)}
                    <span className="flex items-center gap-1.5 text-xs text-muted">
                      {i.state === 'making' ? (
                        <>
                          <Loader2 className="size-3.5 animate-spin text-accent" aria-hidden />
                          {t('En préparation…', 'Being made…')} {i.percent ?? 0} %
                        </>
                      ) : (
                        <>
                          <Clock className="size-3.5" aria-hidden />
                          {t('En attente (un à la fois)…', 'Waiting (one at a time)…')}
                        </>
                      )}
                    </span>
                    {i.state === 'making' && (
                      <span className="mt-1 block h-1 overflow-hidden rounded bg-line">
                        <span className="block h-full bg-accent transition-[width]" style={{ width: `${i.percent ?? 0}%` }} />
                      </span>
                    )}
                  </div>
                )}
                {(i.state === 'waiting' || i.state === 'making') && (
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<Square className="size-3.5" />}
                    aria-label={t(`Arrêter le time-lapse du ${label(i.day)}`, `Stop the time-lapse of ${label(i.day)}`)}
                    disabledReason={removeReason(i.day)}
                    onClick={() => void remove(i.day)}
                  />
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
      <ConfirmDialog
        open={toRemove !== null}
        title={t('Supprimer le time-lapse', 'Delete the time-lapse')}
        message={toRemove ? t(`La vidéo du ${label(toRemove)} sera supprimée. Les enregistrements de ce jour ne changent pas : « Préparer » peut la refaire.`, `The video of ${label(toRemove)} will be deleted. The recordings of that day do not change: « Make » can make it again.`) : null}
        confirmLabel={t('Supprimer', 'Delete')}
        danger
        onCancel={() => setToRemove(null)}
        onConfirm={() => {
          const d = toRemove
          setToRemove(null)
          if (d) void remove(d)
        }}
      />
    </Card>
  )
}
