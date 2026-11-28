import { useEffect, useState } from 'react'
import { snapshotUrl } from '../../api/endpoints'
import { useCapability, useIsDemo } from '../../data/DataContext'
import { demoPicture } from '../../data/demo/fixtures'
import { useInView, usePageVisible } from '../../hooks/useUi'
import { t } from '../../lib/i18n'
import type { Camera } from '../../types/api'
import { CameraCard } from './CameraCard'

/** A small picture of the camera refreshes this often (one JPEG, not video). */
const REFRESH_MS = 15_000

/** Current picture of a camera, refreshed while on screen: no live video, light on a phone. */
function Picture({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const snapshots = useCapability('snapshots') === true
  const [ref, visible] = useInView<HTMLSpanElement>()
  const pageVisible = usePageVisible()
  const [stamp, setStamp] = useState(() => Date.now())
  const [failed, setFailed] = useState<number | null>(null)
  const online = camera.status === 'online'
  useEffect(() => {
    if (demo || !visible || !pageVisible || !online) return
    setStamp(Date.now())
    const timer = window.setInterval(() => setStamp(Date.now()), REFRESH_MS)
    return () => window.clearInterval(timer)
  }, [demo, visible, pageVisible, online])
  const src = demo ? demoPicture(camera.id) : snapshots && online && failed !== stamp ? snapshotUrl(camera.id, stamp) : null
  return (
    <span ref={ref} className="absolute inset-0 block">
      {src && <img src={src} alt="" decoding="async" className="h-full w-full object-cover" onError={() => setFailed(stamp)} />}
    </span>
  )
}

/**
 * The cameras on a phone: one large in live view, the others as small
 * pictures under it; touching one puts it large. Without a choice, the
 * large one is the first camera in motion, else the first online.
 */
export function PhoneCameras({ cameras }: { cameras: Camera[] }) {
  const [picked, setPicked] = useState<string | null>(null)
  const large = cameras.find((c) => c.id === picked) ?? cameras.find((c) => c.motion.active) ?? cameras.find((c) => c.status === 'online') ?? cameras[0]
  if (!large) return null
  const others = cameras.filter((c) => c.id !== large.id)
  return (
    <div className="flex flex-col gap-3">
      <CameraCard camera={large} />
      {others.length > 0 && (
        <ul className="grid grid-cols-3 gap-2">
          {others.map((c) => (
            <li key={c.id}>
              <button type="button" onClick={() => setPicked(c.id)} aria-label={t(`Afficher ${c.name} en grand`, `Show ${c.name} large`)} className="flex w-full flex-col gap-1 text-left">
                <span className={`relative block aspect-video w-full overflow-hidden rounded-lg bg-black ${c.motion.active ? 'ring-2 ring-motion' : ''}`}>
                  <Picture camera={c} />
                  {c.status !== 'online' && <span className="absolute inset-0 flex items-center justify-center bg-black/60 text-[10px] text-white/80">{c.status === 'paused' ? t('En pause', 'Paused') : c.status === 'connecting' ? t('Connexion…', 'Connecting…') : t('Hors ligne', 'Offline')}</span>}
                </span>
                <span className="truncate text-xs">{c.name}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
