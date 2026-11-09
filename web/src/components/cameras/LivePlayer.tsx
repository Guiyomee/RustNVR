import { LoaderCircle, MonitorX, Volume2, VolumeX } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { liveUrl, type LiveStream } from '../../api/endpoints'
import { t } from '../../lib/i18n'

type Status =
  /** `refused`: the server never accepted the connection (a reverse proxy without WebSockets, usually). */
  | { kind: 'connecting'; refused?: boolean }
  | { kind: 'playing'; converted: boolean }
  | { kind: 'unsupported'; codec: string }
  | { kind: 'error'; message: string }

/** Seconds behind the newest buffered frame before jumping forward. */
const MAX_LATENCY_S = 1.5
/** Seconds of video kept behind the playhead. */
const KEEP_BEHIND_S = 10
/** Reconnect if nothing arrives for this long. */
const STALL_MS = 10_000
/** Pending fragments beyond which the browser is considered too slow. */
const MAX_QUEUE = 300

/** Streams this browser could not decode, converted to H.264 from then on. */
const needsConversion = new Set<string>()

function codecLabel(codec: string): string {
  if (codec.startsWith('hvc1') || codec.startsWith('hev1')) return 'H.265'
  if (codec.startsWith('avc1')) return 'H.264'
  return codec
}

/**
 * Live view through Media Source Extensions. The server sends an init
 * message, an MP4 initialization segment and one fragment per frame
 * (`/api/v1/cameras/{id}/live`). Mount it only while the camera is visible:
 * unmounting closes the connection and frees the decoder.
 *
 * If the browser cannot decode the camera codec (H.265 in Firefox) and the
 * server can convert (`canConvert`), the player asks for an H.264 conversion
 * and remembers that choice for this stream.
 */
export function LivePlayer({
  cameraId,
  stream,
  canConvert,
  large = false,
  audio = false,
}: {
  cameraId: string
  stream?: LiveStream
  canConvert: boolean
  large?: boolean
  /** Ask for the camera's sound (muted until the viewer turns it on). */
  audio?: boolean
}) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const [status, setStatus] = useState<Status>({ kind: 'connecting' })
  const [hasAudio, setHasAudio] = useState(false)
  // Muted by default: browsers only start muted videos by themselves.
  const [muted, setMuted] = useState(true)

  useEffect(() => {
    if (videoRef.current) videoRef.current.muted = muted
  }, [muted])

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    if (typeof MediaSource === 'undefined') {
      setStatus({ kind: 'error', message: t('Ce navigateur ne prend pas en charge Media Source Extensions (iPhone notamment).', 'This browser does not support Media Source Extensions (iPhone in particular).') })
      return
    }
    let stopped = false
    const streamKey = `${cameraId}/${stream ?? 'default'}`
    let convert = needsConversion.has(streamKey)
    // Dropped if the browser cannot play this sound with this video.
    let withAudio = audio
    let ws: WebSocket | null = null
    let objectUrl = ''
    let retryTimer: number | undefined
    let delay = 1000
    let lastData = Date.now()
    // Connections closed before opening, in a row.
    let refusals = 0

    const teardown = () => {
      const socket = ws
      ws = null
      socket?.close()
      if (objectUrl) {
        video.removeAttribute('src')
        video.load()
        URL.revokeObjectURL(objectUrl)
        objectUrl = ''
      }
    }

    const scheduleReconnect = () => {
      teardown()
      if (stopped) return
      setStatus({ kind: 'connecting', refused: refusals >= 3 })
      retryTimer = window.setTimeout(connect, delay * (1 - Math.random() * 0.2))
      delay = Math.min(delay * 2, 15_000)
    }

    function connect() {
      const ms = new MediaSource()
      objectUrl = URL.createObjectURL(ms)
      video!.src = objectUrl
      let sb: SourceBuffer | null = null
      const queue: ArrayBuffer[] = []

      const followLiveEdge = () => {
        const b = video!.buffered
        if (b.length === 0) return
        const start = b.start(b.length - 1)
        const end = b.end(b.length - 1)
        if (end - video!.currentTime > MAX_LATENCY_S || video!.currentTime < start) {
          video!.currentTime = Math.max(start, end - 0.3)
        }
        if (video!.paused) video!.play().catch(() => undefined)
      }

      const pump = () => {
        if (!sb || sb.updating) return
        const next = queue.shift()
        if (next) {
          try {
            sb.appendBuffer(next)
          } catch {
            scheduleReconnect() // QuotaExceededError or a closed source
          }
          return
        }
        // Idle: drop what is far behind the playhead to bound memory.
        const b = video!.buffered
        if (b.length > 0 && video!.currentTime - b.start(0) > 3 * KEEP_BEHIND_S) {
          try {
            sb.remove(0, video!.currentTime - KEEP_BEHIND_S)
          } catch {
            // Removal is an optimisation; ignore failures.
          }
        }
      }

      ms.addEventListener(
        'sourceopen',
        () => {
          const socket = new WebSocket(liveUrl(cameraId, { stream, transcode: convert, audio: withAudio }))
          socket.binaryType = 'arraybuffer'
          ws = socket
          let opened = false
          socket.onopen = () => {
            opened = true
            refusals = 0
          }
          socket.onmessage = (ev: MessageEvent) => {
            lastData = Date.now()
            if (typeof ev.data === 'string') {
              type Init = { type?: string; codec?: string; codecs?: string; audio?: string | null }
              let msg: Init
              try {
                msg = JSON.parse(ev.data) as Init
              } catch {
                return
              }
              if (msg.type === 'init' && msg.codec) {
                const video = `video/mp4; codecs="${msg.codec}"`
                const mime = `video/mp4; codecs="${msg.codecs ?? msg.codec}"`
                if (MediaSource.isTypeSupported(video) && !MediaSource.isTypeSupported(mime)) {
                  // The video plays but not with this sound: go on without it.
                  teardown()
                  withAudio = false
                  connect()
                  return
                }
                setHasAudio(Boolean(msg.audio))
                if (!MediaSource.isTypeSupported(mime)) {
                  teardown()
                  if (canConvert && !convert) {
                    // Ask the server for an H.264 conversion instead.
                    convert = true
                    needsConversion.add(streamKey)
                    setStatus({ kind: 'connecting' })
                    connect()
                  } else {
                    stopped = true
                    setStatus({ kind: 'unsupported', codec: msg.codec })
                  }
                  return
                }
                sb = ms.addSourceBuffer(mime)
                sb.mode = 'segments'
                sb.addEventListener('updateend', () => {
                  followLiveEdge()
                  pump()
                })
              }
              return
            }
            queue.push(ev.data as ArrayBuffer)
            if (queue.length > MAX_QUEUE) scheduleReconnect()
            else pump()
          }
          socket.onclose = (ev: CloseEvent) => {
            if (ws !== socket) return
            if (ev.reason.startsWith('transcode:')) {
              // Conversion failure (e.g. FFmpeg missing): retrying will not help.
              stopped = true
              teardown()
              setStatus({ kind: 'error', message: t(`Conversion H.264 impossible (${ev.reason.slice('transcode:'.length).trim()}).`, `H.264 conversion failed (${ev.reason.slice('transcode:'.length).trim()}).`) })
              return
            }
            if (!opened) refusals += 1
            scheduleReconnect()
          }
        },
        { once: true },
      )
    }

    video.onplaying = () => {
      delay = 1000
      setStatus({ kind: 'playing', converted: convert })
    }
    connect()
    const watchdog = window.setInterval(() => {
      if (!stopped && ws && Date.now() - lastData > STALL_MS) scheduleReconnect()
    }, 2_000)

    return () => {
      stopped = true
      window.clearTimeout(retryTimer)
      window.clearInterval(watchdog)
      video.onplaying = null
      teardown()
    }
  }, [cameraId, stream, canConvert, audio])

  return (
    <div className="relative h-full w-full bg-black">
      <video ref={videoRef} muted autoPlay playsInline className="h-full w-full object-contain" />
      {status.kind === 'playing' && hasAudio && (
        <button
          type="button"
          onClick={() => setMuted((m) => !m)}
          aria-label={muted ? t('Activer le son du direct', 'Turn live sound on') : t('Couper le son du direct', 'Mute live sound')}
          title={muted ? t('Activer le son', 'Turn sound on') : t('Couper le son', 'Mute')}
          className="absolute right-2 bottom-2 rounded bg-black/60 p-1.5 text-white hover:bg-black/80"
        >
          {muted ? <VolumeX className="size-4" /> : <Volume2 className="size-4" />}
        </button>
      )}
      {/* No « Live » badge: an online camera is obviously live, and the top
          left corner belongs to the recording state (REC, motion). */}
      {status.kind === 'playing' && status.converted && (
        <div className="pointer-events-none absolute top-2 right-2 flex gap-1">
          {(
            <span
              className="rounded bg-black/60 px-1.5 py-0.5 text-[11px] text-fg/90"
              title={t("Ce navigateur ne décode pas le codec de la caméra : le serveur convertit le direct en H.264 (l'enregistrement reste d'origine).", "This browser cannot decode the camera codec: the server converts the live view to H.264 (the recording stays original).")}
            >
              {t('converti H.264', 'converted H.264')}
            </span>
          )}
        </div>
      )}
      {status.kind !== 'playing' && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-bg-2/80 p-4 text-center">
          {status.kind === 'connecting' ? (
            <>
              <LoaderCircle className={`${large ? 'size-8' : 'size-6'} animate-spin text-muted`} aria-hidden />
              <p className="text-sm text-muted">{t('Connexion au direct…', 'Connecting to the live view…')}</p>
              {status.refused && (
                <p className={`max-w-md text-xs text-warn ${large ? '' : 'line-clamp-3'}`}>
                  {t('Le serveur refuse la connexion du direct (WebSocket). Derrière un mandataire inverse, activez les WebSockets : case « Websockets Support » dans Nginx Proxy Manager.', 'The server refuses the live connection (WebSocket). Behind a reverse proxy, turn WebSockets on: « Websockets Support » in Nginx Proxy Manager.')}
                </p>
              )}
            </>
          ) : (
            <>
              <MonitorX className={`${large ? 'size-9' : 'size-7'} text-warn`} aria-hidden />
              <p className="text-sm font-medium">
                {status.kind === 'unsupported' ? t(`Ce navigateur ne décode pas le ${codecLabel(status.codec)}`, `This browser cannot decode ${codecLabel(status.codec)}`) : t('Direct impossible', 'Live view impossible')}
              </p>
              <p className={`max-w-md text-xs text-muted ${large ? '' : 'line-clamp-3'}`}>
                {status.kind === 'unsupported'
                  ? t(`Codec ${status.codec}. Activez la conversion côté serveur (live.transcode), réglez la caméra en H.264, ou utilisez un navigateur qui décode ce codec.`, `Codec ${status.codec}. Turn on server-side conversion (live.transcode), set the camera to H.264, or use a browser that decodes this codec.`)
                  : status.message}
              </p>
            </>
          )}
        </div>
      )}
    </div>
  )
}
