import { CircleOff, MonitorOff, VideoOff } from 'lucide-react'
import { memo, useEffect, useState } from 'react'
import { useCapability, useIsDemo } from '../../data/DataContext'
import { demoPicture, demoSpec } from '../../data/demo/fixtures'
import { usePageVisible } from '../../hooks/useUi'
import { formatTime } from '../../lib/format'
import type { Camera } from '../../types/api'
import type { LiveStream } from '../../api/endpoints'
import { LivePlayer } from './LivePlayer'
import { MaskOverlay } from './MaskOverlay'
import { WebrtcPlayer } from './WebrtcPlayer'
import { t } from '../../lib/i18n'

/**
 * The single place where live video is rendered.
 *
 * Real mode: the live player connects only while `active` (card visible and
 * browser tab shown) and the camera is online, so hidden cameras cost
 * nothing on the server, the network or the decoder. Demo mode: a drawn
 * placeholder labeled as such, never fake video.
 */
export const CameraPlayer = memo(function CameraPlayer({
  camera,
  active,
  large = false,
  stream,
  lowLatency = false,
}: {
  camera: Camera
  active: boolean
  large?: boolean
  /** Camera stream for the live view; by default the server picks the sub stream when configured. */
  stream?: LiveStream
  /** Use the WebRTC player (experimental, needs a direct UDP path to the server). */
  lowLatency?: boolean
}) {
  const demo = useIsDemo()
  const live = useCapability('live_video')
  const canConvert = useCapability('live_transcode') === true
  const pageVisible = usePageVisible()
  if (demo) return <DemoPreview camera={camera} animate={active} />
  if (live && camera.status === 'online') {
    if (!active || !pageVisible) return <Paused large={large} />
    const player = lowLatency ? (
      <WebrtcPlayer key={camera.online_since_ms ?? 0} cameraId={camera.id} stream={stream} large={large} />
    ) : (
      <LivePlayer
        // Reconnects when the camera restarts, or when its sound is switched.
        key={`${camera.online_since_ms ?? 0}:${camera.record_audio}`}
        cameraId={camera.id}
        stream={stream}
        canConvert={canConvert}
        large={large}
        // Sound only in the large view (camera page), not in the grid.
        audio={large && camera.record_audio && camera.audio !== null}
      />
    )
    const masks = camera.privacy_masks ?? []
    if (masks.length === 0) return player
    return (
      <div className="relative h-full w-full">
        {player}
        <MaskOverlay masks={masks} width={camera.width} height={camera.height} />
      </div>
    )
  }
  return <StatusPanel camera={camera} liveAvailable={live === true} large={large} />
})

function Paused({ large }: { large: boolean }) {
  return (
    <div className="flex h-full w-full items-center justify-center bg-bg-2 p-4 text-center">
      <p className={`text-muted ${large ? 'text-sm' : 'text-xs'}`}>{t("Direct en pause (caméra hors de l'écran ou onglet masqué)", "Live view paused (camera off screen or tab hidden)")}</p>
    </div>
  )
}

function StatusPanel({ camera, liveAvailable, large }: { camera: Camera; liveAvailable: boolean; large: boolean }) {
  const iconCls = large ? 'size-10' : 'size-7'
  let icon = <MonitorOff className={`${iconCls} text-muted`} aria-hidden />
  let title = t('Direct vidéo non disponible', 'Live video not available')
  let detail = liveAvailable
    ? t('Aucune image pour le moment.', 'No picture yet.')
    : t('Ce serveur RustNVR ne diffuse pas le direct (version antérieure). Les enregistrements sont visibles dans « Lecture ».', 'This RustNVR server does not stream the live view (older version). Recordings can be seen in « Playback ».')
  if (camera.status === 'disabled') {
    icon = <CircleOff className={`${iconCls} text-muted`} aria-hidden />
    title = t('Caméra désactivée', 'Camera off')
    detail = t('Activez-la dans config.toml (enabled = true).', 'Turn it on in config.toml (enabled = true).')
  } else if (camera.status !== 'online') {
    icon = <VideoOff className={`${iconCls} text-err`} aria-hidden />
    title = camera.status === 'connecting' ? t('Connexion en cours…', 'Connecting…') : t('Caméra hors ligne', 'Camera offline')
    detail = camera.last_error ?? t('Aucun flux reçu pour le moment.', 'No stream received yet.')
  }
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-2 bg-bg-2 p-4 text-center">
      {icon}
      <p className={`font-medium ${large ? 'text-base' : 'text-sm'}`}>{title}</p>
      <p className={`max-w-md text-muted ${large ? 'text-sm' : 'line-clamp-3 text-xs'}`}>{detail}</p>
    </div>
  )
}

function Clock() {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [])
  return <>{formatTime(now, true)}</>
}

/** Still photo for demo mode (a drawn scene if it is missing). It is not video and says so. */
function DemoPreview({ camera, animate }: { camera: Camera; animate: boolean }) {
  const hue = demoSpec(camera.id)?.hue ?? 210
  const picture = demoPicture(camera.id)
  const [broken, setBroken] = useState(false)
  if (camera.status !== 'online') return <StatusPanel camera={camera} liveAvailable={false} large={false} />
  if (picture && !broken) {
    return (
      <div className="relative h-full w-full overflow-hidden bg-bg-2">
        <img src={picture} alt="" decoding="async" className="absolute inset-0 h-full w-full object-cover" onError={() => setBroken(true)} />
        <div className="absolute right-2 bottom-2 rounded bg-black/55 px-1.5 py-0.5 text-[10px] font-semibold tracking-wide text-warn uppercase" title={t('Photo libre de droits (CC0) : ce n’est pas une caméra.', 'Royalty-free photo (CC0): this is not a camera.')}>
          {t('Image fixe · démo', 'Still picture · demo')}
        </div>
        <div className="absolute bottom-2 left-2 rounded bg-black/55 px-1.5 py-0.5 font-mono text-[10px] text-fg/90 tabular">
          {animate ? <Clock /> : '--:--:--'}
        </div>
      </div>
    )
  }
  return (
    <div className="relative h-full w-full overflow-hidden bg-bg-2">
      <svg viewBox="0 0 160 90" preserveAspectRatio="xMidYMid slice" className="absolute inset-0 h-full w-full" aria-hidden>
        <defs>
          <linearGradient id={`sky-${camera.id}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor={`hsl(${hue} 35% 30%)`} />
            <stop offset="1" stopColor={`hsl(${hue} 25% 14%)`} />
          </linearGradient>
        </defs>
        <rect width="160" height="90" fill={`url(#sky-${camera.id})`} />
        <path d="M0 62 L40 50 L80 58 L120 46 L160 54 L160 90 L0 90 Z" fill={`hsl(${hue} 18% 11%)`} />
        <rect x="18" y="38" width="34" height="26" fill={`hsl(${hue} 12% 20%)`} />
        <rect x="24" y="44" width="8" height="8" fill={`hsl(45 60% ${animate ? 45 : 35}%)`} />
        <rect x="104" y="52" width="40" height="14" rx="3" fill={`hsl(${hue} 10% 24%)`} />
        <path d="M0 90 L70 66 L90 66 L160 90 Z" fill={`hsl(${hue} 8% 17%)`} />
      </svg>
      <div className="absolute inset-0 flex items-center justify-center">
        <span className="rounded bg-black/55 px-2 py-1 text-[10px] font-semibold tracking-widest text-warn uppercase">
          {t('Prévisualisation de démonstration', 'Demo preview')}
        </span>
      </div>
      <div className="absolute bottom-2 left-2 rounded bg-black/55 px-1.5 py-0.5 font-mono text-[10px] text-fg/90 tabular">
        {animate ? <Clock /> : '--:--:--'}
      </div>
    </div>
  )
}
