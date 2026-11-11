import { Film, LoaderCircle, Volume2, VolumeX } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useCapability } from '../../data/DataContext'
import { API_BASE } from '../../api/client'
import { withBase } from '../../lib/build'
import { formatTime } from '../../lib/format'
import type { Recording } from '../../types/api'
import { MaskOverlay } from '../cameras/MaskOverlay'
import { recordingAt } from '../timeline/periods'
import { t } from '../../lib/i18n'

interface Props {
  /** Camera identifier (HLS playlist). */
  camera: string
  name: string
  codec: string | null
  recordings: Recording[]
  /** Shared playback clock (wall-clock ms). */
  position: number
  playing: boolean
  rate: number
  /** The master player drives the clock; others follow it. */
  master: boolean
  onTime?: (ms: number) => void
  /** Master only: the clock is in a gap; `next` is the next recording start, if any. */
  onGap?: (next: number | null) => void
  /** Privacy masks `[x1, y1, x2, y2]` and picture size, to black them out. */
  masks?: number[][]
  width?: number | null
  height?: number | null
}

/** Drift tolerated before a player is re-seeked to the shared clock. */
const MAX_DRIFT_S = 1.5
/** Segments closer than this are played as one continuous block. */
const CONTIGUOUS_MS = 2_000

/** A block of back-to-back segments, played from one HLS playlist. */
interface Run {
  segments: Recording[]
  /** Media time (s) at which each segment starts in the playlist. */
  offsets: number[]
  start: number
  end: number
}

function runs(recordings: Recording[]): Run[] {
  const sorted = [...recordings].filter((r) => r.url).sort((a, b) => a.start_ms - b.start_ms)
  const out: Run[] = []
  for (const r of sorted) {
    const last = out[out.length - 1]
    if (last && r.start_ms - last.end <= CONTIGUOUS_MS) {
      last.offsets.push((last.offsets[last.offsets.length - 1] ?? 0) + (last.segments[last.segments.length - 1]?.duration_ms ?? 0) / 1000)
      last.segments.push(r)
      last.end = Math.max(last.end, r.end_ms)
    } else {
      out.push({ segments: [r], offsets: [0], start: r.start_ms, end: r.end_ms })
    }
  }
  return out
}

/** Media time of a wall-clock instant inside `run`. */
function toMedia(run: Run, ms: number): number {
  for (let i = run.segments.length - 1; i >= 0; i--) {
    const s = run.segments[i]
    if (s && ms >= s.start_ms) return (run.offsets[i] ?? 0) + Math.min(ms - s.start_ms, s.duration_ms) / 1000
  }
  return 0
}

/** Wall-clock instant of a media time of `run`. */
function toWall(run: Run, t: number): number {
  for (let i = run.segments.length - 1; i >= 0; i--) {
    const s = run.segments[i]
    const o = run.offsets[i] ?? 0
    if (s && t >= o) return s.start_ms + (t - o) * 1000
  }
  return run.start
}

type Mode = 'hls.js' | 'native' | 'files' | 'converted'

/** Whether this browser decodes H.265 in MP4 (Safari, Edge with the HEVC extension…). */
export function decodesHevc(): boolean {
  const types = ['video/mp4; codecs="hvc1.1.6.L120.90"', 'video/mp4; codecs="hev1.1.6.L120.90"']
  const probe = document.createElement('video')
  return types.some((t) => (typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(t)) || probe.canPlayType(t) !== '')
}

function hlsMode(): Promise<Mode> {
  const probe = document.createElement('video')
  return import('hls.js').then(
    ({ default: Hls }) => (Hls.isSupported() ? 'hls.js' : probe.canPlayType('application/vnd.apple.mpegurl') ? 'native' : 'files'),
    () => (probe.canPlayType('application/vnd.apple.mpegurl') ? 'native' : 'files'),
  )
}

/**
 * Plays the recordings around `position`. Back-to-back segments are played
 * as one HLS stream (no pause between files): hls.js where Media Source
 * Extensions exist, native HLS on Safari / iPhone, else one file at a time.
 */
export function RecordingPlayer({ camera, name, codec, recordings, position, playing, rate, master, onTime, onGap, masks = [], width = null, height = null }: Props) {
  const video = useRef<HTMLVideoElement>(null)
  const [error, setError] = useState<string | null>(null)
  const [mode, setMode] = useState<Mode | null>(null)
  // The server converts H.265 recordings to H.264 when the browser cannot decode them.
  const canConvert = useCapability('live_transcode') === true
  const [converting, setConverting] = useState(false)
  // Muted by default: browsers only start muted videos by themselves.
  const [muted, setMuted] = useState(true)
  const seg = recordingAt(recordings, position)
  const allRuns = useMemo(() => runs(recordings), [recordings])
  const run = allRuns.find((r) => position >= r.start && position < r.end) ?? null
  const playlist = run ? `${API_BASE}/cameras/${encodeURIComponent(camera)}/playlist.m3u8?from=${Math.floor(run.start)}&to=${Math.ceil(run.end)}` : ''
  // One file at a time: without HLS, or with converted copies.
  const files = mode === 'files' || mode === 'converted'
  const src = mode === 'converted' ? (seg?.url ? `${withBase(seg.url)}?transcode=h264` : '') : files ? (seg?.url ? withBase(seg.url) : '') : playlist
  const unsupported = t(`Ce navigateur ne sait pas décoder ce flux${codec ? ` (${codec.toUpperCase()})` : ''}. Le H.265 dépend du navigateur et du matériel.`, `This browser cannot decode this stream${codec ? ` (${codec.toUpperCase()})` : ''}. H.265 depends on the browser and the hardware.`)
  const media = files ? (seg ? (position - seg.start_ms) / 1000 : 0) : run ? toMedia(run, position) : 0
  const wall = (t: number) => (files ? (seg ? seg.start_ms + t * 1000 : position) : run ? toWall(run, t) : position)
  // Latest values for the media callbacks.
  const latest = useRef({ media, playing, rate })
  latest.current = { media, playing, rate }

  useEffect(() => {
    let alive = true
    if (codec === 'h265' && canConvert && !decodesHevc()) setMode('converted')
    else void hlsMode().then((m) => alive && setMode(m))
    return () => {
      alive = false
    }
  }, [codec, canConvert])

  // Converted copies take a few seconds to prepare.
  useEffect(() => setConverting(mode === 'converted' && src !== ''), [mode, src])

  useEffect(() => {
    if (video.current) video.current.muted = muted
  }, [muted, mode, src])

  // Master in a gap: ask the page to jump to the next recording.
  useEffect(() => {
    if (!master || seg || !playing) return
    const next = recordings.filter((r) => r.start_ms > position).sort((a, b) => a.start_ms - b.start_ms)[0]
    onGap?.(next?.start_ms ?? null)
  }, [master, seg, playing, position, recordings, onGap])

  useEffect(() => setError(null), [src])

  // hls.js: one instance per block.
  useEffect(() => {
    const v = video.current
    if (mode !== 'hls.js' || !v || !playlist) return
    let hls: import('hls.js').default | null = null
    let alive = true
    void import('hls.js').then(({ default: Hls }) => {
      if (!alive) return
      hls = new Hls({ maxBufferLength: 30, enableWorker: true })
      hls.on(Hls.Events.MANIFEST_PARSED, () => {
        const { media: t, playing: p, rate: r } = latest.current
        v.playbackRate = r
        v.currentTime = Math.max(0, t)
        if (p) v.play().catch(() => undefined)
      })
      hls.on(Hls.Events.ERROR, (_, data) => {
        if (!data.fatal) return
        const codecError = data.details === Hls.ErrorDetails.BUFFER_ADD_CODEC_ERROR || data.details === Hls.ErrorDetails.BUFFER_INCOMPATIBLE_CODECS_ERROR
        if (codecError && canConvert) setMode('converted')
        else setError(codecError ? unsupported : t(`Lecture impossible (${data.details}).`, `Playback failed (${data.details}).`))
      })
      hls.loadSource(playlist)
      hls.attachMedia(v)
    })
    return () => {
      alive = false
      hls?.destroy()
    }
  }, [mode, playlist, unsupported, canConvert])

  // Follow the shared clock (user seeks for the master, sync for the others).
  useEffect(() => {
    const v = video.current
    if (!v || !src || v.readyState < 1) return
    if (Math.abs(v.currentTime - media) > MAX_DRIFT_S) v.currentTime = Math.max(0, media)
  }, [media, src])

  useEffect(() => {
    const v = video.current
    if (!v || !src) return
    if (playing) v.play().catch(() => undefined)
    else v.pause()
  }, [playing, src])

  useEffect(() => {
    const v = video.current
    if (!v) return
    try {
      v.playbackRate = rate
    } catch {
      setError(t(`Vitesse ×${rate} non prise en charge par ce navigateur.`, `Speed ×${rate} not supported by this browser.`))
    }
  }, [rate, src])

  if (!seg) {
    return (
      <Frame name={name} label={formatTime(position, true)}>
        <div className="flex h-full flex-col items-center justify-center gap-1 text-center text-sm text-muted">
          <Film className="size-6" aria-hidden />
          {t("Pas d'enregistrement à cet instant", "No recording at this time")}
        </div>
      </Frame>
    )
  }
  if (!seg.url) {
    return (
      <Frame name={name} label={formatTime(position, true)}>
        <div className="flex h-full flex-col items-center justify-center gap-1 p-4 text-center text-sm text-muted">
          <Film className="size-6" aria-hidden />
          {t('Enregistrement fictif (mode démo) : aucune vidéo à lire.', 'Fictional recording (demo mode): no video to play.')}
        </div>
      </Frame>
    )
  }
  if (!mode) return <Frame name={name} label={formatTime(position, true)}>{null}</Frame>
  return (
    <Frame name={name} label={formatTime(position, true)} overlay={<MaskOverlay masks={masks} width={width} height={height} />}>
      <video
        ref={video}
        key={`${mode}:${src}`}
        // hls.js attaches the stream itself; native HLS and files use src.
        src={mode === 'hls.js' ? undefined : src}
        muted
        playsInline
        preload="auto"
        className="h-full w-full bg-black object-contain"
        onLoadedMetadata={(e) => {
          if (mode === 'hls.js') return
          const v = e.currentTarget
          v.playbackRate = rate
          v.currentTime = Math.max(0, media)
          if (playing) v.play().catch(() => undefined)
          // Prepare the next file while this one plays.
          if (mode === 'converted') {
            const next = recordings.filter((r) => r.url && r.start_ms >= seg.end_ms).sort((a, b) => a.start_ms - b.start_ms)[0]
            if (next?.url) void fetch(`${withBase(next.url)}?transcode=h264`, { method: 'HEAD' }).catch(() => undefined)
          }
        }}
        onLoadedData={() => setConverting(false)}
        onTimeUpdate={(e) => master && !e.currentTarget.seeking && onTime?.(wall(e.currentTarget.currentTime))}
        onEnded={() => master && onTime?.(files ? (seg.end_ms) : (run?.end ?? seg.end_ms))}
        onError={(e) => {
          if (mode === 'hls.js') return
          const code = e.currentTarget.error?.code
          setConverting(false)
          if (code === 4 && mode !== 'converted' && canConvert) setMode('converted')
          else if (mode === 'converted') setError(t('Conversion H.264 impossible sur le serveur (voir le journal de RustNVR).', 'H.264 conversion failed on the server (see the RustNVR log).'))
          else setError(code === 4 ? unsupported : t('Lecture impossible (fichier inaccessible ou réseau).', 'Playback failed (file unreachable or network).'))
        }}
      />
      <button
        type="button"
        onClick={() => setMuted((m) => !m)}
        aria-label={muted ? t(`Activer le son de ${name}`, `Turn on the sound of ${name}`) : t(`Couper le son de ${name}`, `Mute ${name}`)}
        title={muted ? t("Activer le son (si l'enregistrement en contient)", "Turn sound on (if the recording has some)") : t('Couper le son', 'Mute')}
        className="absolute right-2 bottom-2 rounded bg-black/60 p-1.5 text-white hover:bg-black/80"
      >
        {muted ? <VolumeX className="size-4" /> : <Volume2 className="size-4" />}
      </button>
      {converting && !error && (
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center gap-2 bg-black/60 text-center text-sm text-muted">
          <LoaderCircle className="size-6 animate-spin" aria-hidden />
          {t('Conversion H.264 pour ce navigateur…', 'H.264 conversion for this browser…')}
          <span className="text-xs">{t('quelques secondes par minute de vidéo', 'a few seconds per minute of video')}</span>
        </div>
      )}
      {mode === 'converted' && !converting && (
        <div className="pointer-events-none absolute bottom-2 left-2 rounded bg-black/60 px-2 py-0.5 text-[11px] text-muted" title={t("Ce navigateur ne décode pas le H.265 : le serveur convertit l'enregistrement (l'original reste intact).", "This browser cannot decode H.265: the server converts the recording (the original stays intact).")}>
          {t('converti H.264', 'converted H.264')}
        </div>
      )}
      {error && <div className="absolute inset-x-0 bottom-0 bg-err/85 px-3 py-1.5 text-xs text-white">{error}</div>}
      {!files && run && run.segments.length > 1 && (
        <div className="pointer-events-none absolute bottom-2 left-2 rounded bg-black/60 px-2 py-0.5 text-[11px] text-muted">HLS · {run.segments.length}{t(' segments enchaînés', ' segments in a row')}</div>
      )}
    </Frame>
  )
}

function Frame({ name, label, children, overlay = null }: { name: string; label: string; children: ReactNode; overlay?: ReactNode }) {
  return (
    <div className="card relative aspect-video overflow-hidden">
      {children}
      {overlay}
      <div className="pointer-events-none absolute top-2 left-2 rounded bg-black/60 px-2 py-0.5 text-xs font-medium">{name}</div>
      <div className="pointer-events-none absolute top-2 right-2 rounded bg-black/60 px-2 py-0.5 font-mono text-xs tabular">{label}</div>
    </div>
  )
}
