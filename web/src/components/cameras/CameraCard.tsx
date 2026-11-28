import { GripVertical, Maximize, Moon } from 'lucide-react'
import { memo, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { useCapability, useIsDemo } from '../../data/DataContext'
import { useInView } from '../../hooks/useUi'
import { formatBitrate, formatNumber, formatResolution } from '../../lib/format'
import { Link, navigate } from '../../lib/router'
import type { Camera } from '../../types/api'
import { IconButton } from '../ui/Button'
import { Badge } from '../ui/Card'
import { CameraStatusBadge } from '../ui/StatusIndicator'
import { CameraPlayer } from './CameraPlayer'
import { RecordingBadge } from './RecordingBadge'
import { t } from '../../lib/i18n'

function fullscreenReason(demo: boolean, live: boolean | null, camera: Camera): string | null {
  if (demo) return t('Aucune vidéo à agrandir en mode démo.', 'No video to enlarge in demo mode.')
  if (!live) return t("Le direct vidéo n'est pas encore disponible.", "Live video is not available yet.")
  if (camera.status !== 'online') return t('Caméra hors ligne.', 'Camera offline.')
  return null
}

/** A tile is video: dark in both themes (its texts are white on the picture). */
const DARK_TILE = {
  '--color-bg': '#0d0f12',
  '--color-bg-2': '#0a0c0e',
  '--color-card': '#14171b',
  '--color-line': '#252a31',
  '--color-fg': '#e8eaed',
  '--color-muted': '#8b93a1',
  '--color-warn': '#f59e0b',
  '--color-err': '#ef5350',
  '--color-ok': '#3fb97a',
} as CSSProperties

export const CameraCard = memo(function CameraCard({ camera, handle }: { camera: Camera; handle?: ReactNode }) {
  const demo = useIsDemo()
  const live = useCapability('live_video')
  const [ref, visible] = useInView<HTMLDivElement>()
  const href = `/cameras/${encodeURIComponent(camera.id)}`
  const reason = fullscreenReason(demo, live, camera)

  const motion = camera.motion.active
  return (
    <article
      ref={ref}
      className="group @container relative aspect-video min-w-0 overflow-hidden rounded-[var(--radius-card)] bg-black text-fg"
      style={DARK_TILE}
    >
      <div
        role="link"
        tabIndex={0}
        aria-label={t(`Ouvrir ${camera.name}`, `Open ${camera.name}`)}
        onClick={() => navigate(href)}
        onKeyDown={(e) => e.key === 'Enter' && navigate(href)}
        className="absolute inset-0 cursor-pointer"
      >
        <CameraPlayer camera={camera} active={visible} />
      </div>
      {/* Name and badges over the picture, on a shade that keeps them readable. */}
      <div className="pointer-events-none absolute inset-0 flex flex-col justify-between bg-[linear-gradient(rgba(0,0,0,0.55),rgba(0,0,0,0)_35%,rgba(0,0,0,0)_65%,rgba(0,0,0,0.6))] p-2.5">
        <div className="flex items-center gap-2">
          {handle && <span className="pointer-events-auto text-white/80">{handle}</span>}
          <Link to={href} className="pointer-events-auto min-w-0 truncate text-sm font-semibold text-white hover:underline">
            {camera.name}
          </Link>
          {camera.night && <Moon className="size-3.5 shrink-0 text-white/80" aria-label={t('Vision nocturne', 'Night vision')} />}
          <span className="flex-1" />
          {demo && <Badge tone="demo">{t('Démo', 'Demo')}</Badge>}
          {motion && <span className="rounded-md bg-motion px-2 py-0.5 text-[11px] font-bold tracking-wide text-[#1a1407] uppercase">{t('Mouvement', 'Motion')}</span>}
          {/* Badges on one line each; on a narrow card, a camera that is not
              online shows only its state (« Hors ligne » says enough). */}
          {!demo && (
            <span className={`shrink-0 whitespace-nowrap ${camera.status !== 'online' ? 'hidden @[24rem]:inline' : ''}`}>
              <RecordingBadge camera={camera} overlay />
            </span>
          )}
          {camera.status !== 'online' && (
            <span className="shrink-0 whitespace-nowrap rounded bg-black/60 px-1.5 py-0.5">
              <CameraStatusBadge status={camera.status} demo={demo} />
            </span>
          )}
        </div>
        <div className="flex items-center justify-end gap-2 font-mono text-[11px] text-white/90">
          {/* Left free: the live view writes its time there. */}
          {camera.status === 'online' && (
            <>
              <span className="hidden rounded bg-black/55 px-1.5 py-0.5 whitespace-nowrap @[26rem]:inline" title={t('Images par seconde mesurées · débit reçu', 'Measured frames per second · received bitrate')}>
                {camera.fps === null ? t('— i/s', '— fps') : t(`${formatNumber(camera.fps, 1)} i/s`, `${formatNumber(camera.fps, 1)} fps`)} · {formatBitrate(camera.bitrate_kbps)}
              </span>
              <span className="rounded bg-black/55 px-1.5 py-0.5 whitespace-nowrap" title={t('Codec', 'Codec')}>{camera.codec?.toUpperCase() ?? '—'}</span>
              <span className="rounded bg-black/55 px-1.5 py-0.5 whitespace-nowrap" title={t('Résolution', 'Resolution')}>{formatResolution(camera.width, camera.height)}</span>
            </>
          )}
          <span className="pointer-events-auto rounded-md bg-black/55 text-white">
            <IconButton label={t('Plein écran', 'Full screen')} icon={<Maximize className="size-4" />} disabledReason={reason} onClick={() => void ref.current?.requestFullscreen().catch(() => undefined)} />
          </span>
        </div>
      </div>
      {motion && <div className="pointer-events-none absolute inset-0 rounded-[var(--radius-card)] border-2 border-motion" aria-hidden />}
    </article>
  )
})

/** Cameras per row: `auto` follows the screen width; a number is a maximum (small screens show fewer). */
export type GridColumns = 'auto' | 1 | 2 | 3 | 4

const GRID_CLASS: Record<GridColumns, string> = {
  auto: 'md:grid-cols-2 min-[1500px]:grid-cols-3 min-[2400px]:grid-cols-4',
  1: '',
  2: 'md:grid-cols-2',
  3: 'md:grid-cols-2 xl:grid-cols-3',
  4: 'md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4',
}

/** Grip of a card: drag it to move the camera, or use the arrow keys. */
function DragHandle({ name, onArm, onStep }: { name: string; onArm: () => void; onStep: (delta: -1 | 1) => void }) {
  return (
    // Not a <button>: Firefox does not start a drag from a button.
    <span
      role="button"
      tabIndex={0}
      aria-label={t(`Déplacer ${name} (flèches gauche et droite)`, `Move ${name} (left and right arrows)`)}
      title={t('Glisser pour déplacer la caméra (ou flèches ← →)', 'Drag to move the camera (or ← → arrows)')}
      onPointerDown={onArm}
      onKeyDown={(e) => {
        const delta = e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : 0
        if (delta === 0) return
        e.preventDefault()
        onStep(delta)
      }}
      className="-ml-1 flex shrink-0 cursor-grab touch-none items-center rounded p-0.5 text-muted hover:bg-bg-2 hover:text-fg active:cursor-grabbing"
    >
      <GripVertical className="size-4" aria-hidden />
    </span>
  )
}

/** `ids` with `id` moved to the place of `target` (after it when moving forward). */
export function moveId(ids: string[], id: string, target: string): string[] {
  const from = ids.indexOf(id)
  const to = ids.indexOf(target)
  if (from < 0 || to < 0 || from === to) return ids
  const rest = ids.filter((x) => x !== id)
  const at = rest.indexOf(target) + (from < to ? 1 : 0)
  return [...rest.slice(0, at), id, ...rest.slice(at)]
}

/**
 * Camera tiles. With `onReorder`, each tile can be dragged by its grip to
 * another place (`onReorder` receives the new order of the identifiers).
 */
export function CameraGrid({ cameras, columns = 'auto', onReorder }: { cameras: Camera[]; columns?: GridColumns; onReorder?: (ids: string[]) => void }) {
  const [dragged, setDragged] = useState<string | null>(null)
  const [over, setOver] = useState<string | null>(null)
  // Tile whose grip was pressed: dragging starts only from the grip.
  const armed = useRef<string | null>(null)
  const ids = cameras.map((c) => c.id)
  const end = () => {
    armed.current = null
    setDragged(null)
    setOver(null)
  }
  const step = (id: string, delta: -1 | 1) => {
    const target = ids[ids.indexOf(id) + delta]
    if (target && onReorder) onReorder(moveId(ids, id, target))
  }
  return (
    <div className={`grid grid-cols-1 gap-3 ${GRID_CLASS[columns]}`}>
      {cameras.map((c) =>
        onReorder ? (
          <div
            key={c.id}
            draggable
            onDragStart={(e) => {
              if (armed.current !== c.id) {
                e.preventDefault()
                return
              }
              e.dataTransfer.effectAllowed = 'move'
              e.dataTransfer.setData('text/plain', c.id)
              setDragged(c.id)
            }}
            onDragOver={(e) => {
              if (!dragged) return
              e.preventDefault()
              e.dataTransfer.dropEffect = 'move'
              if (over !== c.id) setOver(c.id)
            }}
            onDrop={(e) => {
              e.preventDefault()
              if (dragged && dragged !== c.id) onReorder(moveId(ids, dragged, c.id))
              end()
            }}
            onDragEnd={end}
            onPointerUp={() => (armed.current = null)}
            className={`min-w-0 rounded-[var(--radius-card)] transition-opacity ${dragged === c.id ? 'opacity-40' : ''} ${over === c.id && dragged !== c.id ? 'ring-2 ring-accent' : ''}`}
          >
            <CameraCard camera={c} handle={<DragHandle name={c.name} onArm={() => (armed.current = c.id)} onStep={(d) => step(c.id, d)} />} />
          </div>
        ) : (
          <CameraCard key={c.id} camera={c} />
        ),
      )}
    </div>
  )
}
