import { ChevronLeft, ChevronRight, CirclePlay, Film, Lock, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { describeError } from '../api/client'
import { deleteRecording, getProtected, getRecordings } from '../api/endpoints'
import { useAuth, useReadOnly } from '../auth/AuthGate'
import { decodesHevc } from '../components/playback/RecordingPlayer'
import { Button } from '../components/ui/Button'
import { Card, PageHeader } from '../components/ui/Card'
import { ConfirmDialog, Modal } from '../components/ui/Modal'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { useCapability, useIsDemo, useLive } from '../data/DataContext'
import { useAsync } from '../hooks/useAsync'
import { useEvents } from '../hooks/useEvents'
import { withBase } from '../lib/build'
import { formatBytes, formatDuration, formatTime } from '../lib/format'
import { t } from '../lib/i18n'
import { navigate } from '../lib/router'
import type { NvrEvent, ProtectedRange, Recording } from '../types/api'

const DAY = 86_400_000

/** `YYYY-MM-DD` of the local day of `ms`. */
function dayOf(ms: number): string {
  const d = new Date(ms)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

/** Start of the local day `day` (`YYYY-MM-DD`). */
function dayStart(day: string): number {
  const [y, m, d] = day.split('-').map(Number)
  return new Date(y ?? 1970, (m ?? 1) - 1, d ?? 1).getTime()
}

/**
 * Pictures of the recordings load two at a time, only on screen, and not
 * while a video is open: a browser keeps 6 connections per server, and
 * pictures still being made would hold them all, so the video waited.
 */
const thumbs = { active: 0, paused: false, waiting: [] as (() => void)[] }

function nextThumb() {
  while (!thumbs.paused && thumbs.active < 2 && thumbs.waiting.length > 0) {
    thumbs.active++
    thumbs.waiting.shift()?.()
  }
}

function pauseThumbs(paused: boolean) {
  thumbs.paused = paused
  nextThumb()
}

/** The picture of a recording, loaded when its turn comes. */
function Thumb({ src }: { src: string }) {
  const [box, setBox] = useState<HTMLDivElement | null>(null)
  const [go, setGo] = useState(false)
  const [done, setDone] = useState(false)
  useEffect(() => {
    if (!box || go) return
    const start = () => setGo(true)
    const observer = new IntersectionObserver(
      ([entry]) => {
        const i = thumbs.waiting.indexOf(start)
        if (entry?.isIntersecting) {
          if (i < 0) thumbs.waiting.push(start)
        } else if (i >= 0) thumbs.waiting.splice(i, 1)
        nextThumb()
      },
      { rootMargin: '200px' },
    )
    observer.observe(box)
    return () => {
      observer.disconnect()
      const i = thumbs.waiting.indexOf(start)
      if (i >= 0) thumbs.waiting.splice(i, 1)
    }
  }, [box, go])
  // Frees the slot once loaded, failed, or gone before the end.
  useEffect(() => {
    if (!go || done) return
    return () => {
      thumbs.active--
      nextThumb()
    }
  }, [go, done])
  return (
    <div ref={setBox} className="h-full w-full">
      {go && <img src={src} alt="" decoding="async" className="h-full w-full object-cover" onLoad={() => setDone(true)} onError={(e) => ((e.currentTarget.style.visibility = 'hidden'), setDone(true))} />}
    </div>
  )
}

/** A recording, with what lies in it. */
interface Item {
  rec: Recording
  events: number
  locked: boolean
}

function overlaps(a0: number, a1: number, b0: number, b1: number): boolean {
  return a0 < b1 && b0 < a1
}

/** The recording opened large: played, deleted, or opened in Lecture. */
function Viewer({ items, index, codec, onIndex, onClose, onDelete, admin }: { items: Item[]; index: number; codec: string | null; onIndex: (i: number) => void; onClose: () => void; onDelete: (it: Item) => void; admin: boolean }) {
  const it = items[index]
  const canConvert = useCapability('live_transcode') === true
  // H.265 that this browser cannot decode: its H.264 copy, made by the server.
  const convert = codec === 'h265' && !decodesHevc() && canConvert
  const deleteReason = !admin ? t('Réservé aux administrateurs.', 'Administrators only.') : it?.locked ? t('Période protégée : retirez d’abord sa protection (Lecture).', 'Protected period: remove its protection first (Playback).') : null
  useEffect(() => {
    pauseThumbs(true)
    return () => pauseThumbs(false)
  }, [])
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return
      if (e.key === 'ArrowLeft' && index > 0) onIndex(index - 1)
      else if (e.key === 'ArrowRight' && index < items.length - 1) onIndex(index + 1)
      else if ((e.key === 'Delete' || e.key === 'Backspace') && it && !deleteReason) {
        e.preventDefault()
        onDelete(it)
      }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [index, items.length, onIndex, onDelete, it, deleteReason])
  if (!it) return null
  const { rec } = it
  return (
    <Modal
      open
      wide
      title={`${formatTime(rec.start_ms)} → ${formatTime(rec.end_ms)} · ${formatDuration(rec.duration_ms)} · ${formatBytes(rec.bytes)}`}
      onClose={onClose}
      footer={
        <div className="flex w-full flex-wrap items-center gap-2">
          {/* Newest first: « Précédente » is the later recording. */}
          <Button size="sm" icon={<ChevronLeft className="size-4" />} disabledReason={index === 0 ? t('Première vidéo.', 'First video.') : null} onClick={() => onIndex(index - 1)}>
            {t('Précédente', 'Previous')}
          </Button>
          <span className="tabular px-1 text-xs text-muted">
            {index + 1} / {items.length}
          </span>
          <Button size="sm" icon={<ChevronRight className="size-4" />} disabledReason={index >= items.length - 1 ? t('Dernière vidéo.', 'Last video.') : null} onClick={() => onIndex(index + 1)}>
            {t('Suivante', 'Next')}
          </Button>
          <span className="flex-1" />
          <Button size="sm" icon={<CirclePlay className="size-4" />} onClick={() => navigate(`/playback?camera=${encodeURIComponent(rec.camera)}&t=${rec.start_ms}`)}>
            {t('Ouvrir dans Lecture', 'Open in Playback')}
          </Button>
          <Button size="sm" variant="danger" icon={<Trash2 className="size-4" />} disabledReason={deleteReason} onClick={() => onDelete(it)}>
            {t('Supprimer', 'Delete')}
          </Button>
        </div>
      }
    >
      <div className="space-y-2">
        <video key={rec.url} src={withBase(rec.url) + (convert ? '?transcode=h264' : '')} controls autoPlay muted playsInline className="mx-auto block max-h-[58vh] w-full rounded-md bg-black" />
        <p className="text-xs text-muted">
          {it.events > 0 ? t(`${it.events} événement(s) dans cette vidéo. `, `${it.events} event(s) in this video. `) : ''}
          {convert ? t('Convertie en H.264 pour ce navigateur (quelques secondes au premier affichage). ', 'Converted to H.264 for this browser (a few seconds the first time). ') : ''}
          {t('Clavier : ← → vidéos, Suppr supprimer.', 'Keyboard: ← → videos, Delete delete.')}
        </p>
      </div>
    </Modal>
  )
}

/** « Vidéos »: the recordings of a camera for a day, as pictures, to watch or delete. */
export function VideosPage() {
  const demo = useIsDemo()
  const cameras = useLive((s) => s.cameras).filter((c) => c.enabled)
  const [camera, setCamera] = useState<string | null>(null)
  const cam = camera && cameras.some((c) => c.id === camera) ? camera : (cameras[0]?.id ?? null)
  const codec = cameras.find((c) => c.id === cam)?.codec ?? null
  const [day, setDay] = useState(() => dayOf(Date.now()))
  const from = dayStart(day)
  const to = from + DAY
  const recordings = useAsync((signal) => (cam && !demo ? getRecordings(cam, from, to, signal) : Promise.resolve([] as Recording[])), [cam, from, demo])
  const protectedRanges = useAsync(() => (demo ? Promise.resolve([] as ProtectedRange[]) : getProtected().catch(() => [] as ProtectedRange[])), [demo])
  const query = useMemo(() => ({ camera: cam, fromMs: from, toMs: to, kinds: null }), [cam, from, to])
  const events = useEvents(query)
  const [onlyEvents, setOnlyEvents] = useState(false)
  const [open, setOpen] = useState<number | null>(null)
  const [toDelete, setToDelete] = useState<Item | null>(null)
  const [error, setError] = useState<string | null>(null)
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !readOnly && (!user || user.role === 'admin')

  const items: Item[] = useMemo(() => {
    const evs: NvrEvent[] = events.data ?? []
    const locks = (protectedRanges.data ?? []).filter((p) => p.camera === cam)
    return [...(recordings.data ?? [])]
      .sort((a, b) => b.start_ms - a.start_ms)
      .map((rec) => ({
        rec,
        events: evs.filter((e) => overlaps(e.start_ms, e.end_ms ?? Date.now(), rec.start_ms, rec.end_ms)).length,
        locked: locks.some((p) => overlaps(p.start_ms, p.end_ms, rec.start_ms, rec.end_ms)),
      }))
      .filter((i) => !onlyEvents || i.events > 0)
  }, [recordings.data, events.data, protectedRanges.data, cam, onlyEvents])
  const total = items.reduce((n, i) => n + i.rec.bytes, 0)

  const remove = async (it: Item) => {
    setToDelete(null)
    setError(null)
    const at = items.indexOf(it)
    try {
      await deleteRecording(it.rec.url)
      // The next one takes its place; none left: the viewer closes.
      if (open !== null) setOpen(items.length <= 1 ? null : Math.min(at, items.length - 2))
      recordings.reload()
    } catch (e) {
      setError(describeError(e))
    }
  }

  const header = (
    <PageHeader
      title={t('Vidéos', 'Videos')}
      description={t('Les enregistrements d’une caméra, jour par jour. Cliquez sur une vidéo pour la regarder ; la corbeille la supprime (sauf dans une période protégée).', 'The recordings of a camera, day by day. Click a video to watch it; the trash deletes it (except in a protected period).')}
    />
  )
  if (demo) {
    return (
      <>
        {header}
        <Card>
          <EmptyState title={t('Non disponible en mode démo', 'Not available in demo mode')} icon={<Film className="size-7" />} />
        </Card>
      </>
    )
  }
  return (
    <>
      {header}
      <div className="space-y-4">
        <div className="flex flex-wrap items-end gap-3">
          <label className="text-sm">
            {t('Caméra', 'Camera')}
            <select className="mt-1 block h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={cam ?? ''} onChange={(e) => (setCamera(e.target.value), setOpen(null))}>
              {cameras.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </label>
          <label className="text-sm">
            {t('Jour', 'Day')}
            <input type="date" className="mt-1 block h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={day} max={dayOf(Date.now())} onChange={(e) => e.target.value && (setDay(e.target.value), setOpen(null))} />
          </label>
          <label className="flex h-9 items-center gap-2 text-sm">
            <input type="checkbox" className="size-4 accent-accent" checked={onlyEvents} onChange={(e) => setOnlyEvents(e.target.checked)} />
            {t('Seulement avec un événement', 'Only with an event')}
          </label>
          <span className="ml-auto h-9 text-xs leading-9 text-muted">
            {items.length} {t('vidéos', 'videos')} · {formatBytes(total)}
          </span>
        </div>
        {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
        {recordings.error ? (
          <ErrorState error={recordings.error} onRetry={recordings.reload} />
        ) : recordings.loading && !recordings.data ? (
          <LoadingState />
        ) : items.length === 0 ? (
          <Card>
            <EmptyState title={t('Aucune vidéo ce jour-là', 'No video that day')} icon={<Film className="size-7" />} />
          </Card>
        ) : (
          <ul className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-4">
            {items.map((it, i) => (
              <li key={it.rec.url} className="group relative">
                {admin && (
                  <button
                    type="button"
                    disabled={it.locked}
                    aria-label={t(`Supprimer la vidéo de ${formatTime(it.rec.start_ms)}`, `Delete the video of ${formatTime(it.rec.start_ms)}`)}
                    title={it.locked ? t('Période protégée : retirez d’abord sa protection (Lecture).', 'Protected period: remove its protection first (Playback).') : t('Supprimer cette vidéo', 'Delete this video')}
                    onClick={() => setToDelete(it)}
                    className="absolute top-2 right-2 z-10 flex size-8 items-center justify-center rounded-md bg-black/60 text-white opacity-0 transition-opacity group-hover:opacity-100 hover:bg-err focus-visible:opacity-100 disabled:cursor-not-allowed disabled:hover:bg-black/60"
                  >
                    <Trash2 className="size-4" aria-hidden />
                  </button>
                )}
                <button type="button" className="card block w-full overflow-hidden text-left" onClick={() => setOpen(i)}>
                  <div className="relative aspect-video bg-black">
                    <Thumb src={`${withBase(it.rec.url)}?thumbnail=1`} />
                    <span className="absolute bottom-2 left-2 rounded bg-black/60 px-1.5 py-0.5 font-mono text-[11px] text-white">{formatDuration(it.rec.duration_ms)}</span>
                    {it.locked && (
                      <span className="absolute top-2 left-2 flex items-center gap-1 rounded bg-black/60 px-1.5 py-0.5 text-[11px] text-white">
                        <Lock className="size-3" aria-hidden />
                        {t('Protégée', 'Protected')}
                      </span>
                    )}
                    {it.events > 0 && <span className="absolute right-2 bottom-2 rounded bg-motion px-1.5 py-0.5 text-[11px] font-bold text-[#1a1407]">{t(`${it.events} évén.`, `${it.events} ev.`)}</span>}
                  </div>
                  <div className="flex items-center justify-between px-2 py-1.5 text-xs">
                    <span className="font-mono">
                      {formatTime(it.rec.start_ms)} → {formatTime(it.rec.end_ms)}
                    </span>
                    <span className="text-muted">{formatBytes(it.rec.bytes)}</span>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <ConfirmDialog
        open={toDelete !== null}
        danger
        title={t('Supprimer cette vidéo ?', 'Delete this video?')}
        message={
          toDelete
            ? t(
                `La vidéo de ${formatTime(toDelete.rec.start_ms)} à ${formatTime(toDelete.rec.end_ms)} est effacée pour de bon de ce stockage. Les événements restent, sans cette vidéo.`,
                `The video from ${formatTime(toDelete.rec.start_ms)} to ${formatTime(toDelete.rec.end_ms)} is erased for good from this storage. The events stay, without this video.`,
              )
            : ''
        }
        confirmLabel={t('Supprimer', 'Delete')}
        onConfirm={() => toDelete && void remove(toDelete)}
        onCancel={() => setToDelete(null)}
      />
      {open !== null && <Viewer items={items} index={open} codec={codec} onIndex={setOpen} onClose={() => setOpen(null)} onDelete={setToDelete} admin={admin} />}
    </>
  )
}
