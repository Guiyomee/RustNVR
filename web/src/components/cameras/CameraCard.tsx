import { GripVertical, Maximize, Moon } from 'lucide-react'
import { memo, useRef, useState, type ReactNode } from 'react'
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

export const CameraCard = memo(function CameraCard({ camera, handle }: { camera: Camera; handle?: ReactNode }) {
  const demo = useIsDemo()
  const live = useCapability('live_video')
  const [ref, visible] = useInView<HTMLDivElement>()
  const href = `/cameras/${encodeURIComponent(camera.id)}`
  const reason = fullscreenReason(demo, live, camera)

  return (
    <article className="card group flex min-w-0 flex-col overflow-hidden">
      <header className="flex items-center justify-between gap-2 px-3 py-2.5">
        <div className="flex min-w-0 items-center gap-1.5">
          {handle}
          <Link to={href} className="min-w-0 truncate text-sm font-semibold hover:text-accent">
            {camera.name}
          </Link>
        </div>
        <span className="flex items-center gap-2">
          {camera.night && <Moon className="size-3.5 text-muted" aria-label={t('Vision nocturne', 'Night vision')} />}
          <CameraStatusBadge status={camera.status} demo={demo} />
        </span>
      </header>
      <div
        ref={ref}
        role="link"
        tabIndex={0}
        aria-label={t(`Ouvrir ${camera.name}`, `Open ${camera.name}`)}
        onClick={() => navigate(href)}
        onKeyDown={(e) => e.key === 'Enter' && navigate(href)}
        className="relative aspect-video cursor-pointer bg-bg-2"
      >
        <CameraPlayer camera={camera} active={visible} />
        <div className="pointer-events-none absolute top-2 left-2 flex gap-1">
          {demo && <Badge tone="demo">{t('Démo', 'Demo')}</Badge>}
          {!demo && <RecordingBadge camera={camera} overlay />}
          {camera.motion.active && <Badge tone="warn">{t('Mouvement', 'Motion')}</Badge>}
        </div>
      </div>
      <footer className="flex items-center justify-between gap-2 px-3 py-2 text-xs text-muted">
        <div className="flex min-w-0 flex-wrap gap-x-3 gap-y-0.5 tabular">
          <span title={t('Résolution', 'Resolution')}>{formatResolution(camera.width, camera.height)}</span>
          <span title={t('Images par seconde mesurées', 'Measured frames per second')}>{camera.fps === null ? t('— i/s', '— fps') : t(`${formatNumber(camera.fps, 1)} i/s`, `${formatNumber(camera.fps, 1)} fps`)}</span>
          <span title={t('Codec', 'Codec')}>{camera.codec?.toUpperCase() ?? '—'}</span>
          <span title={t('Débit reçu', 'Received bitrate')} className="hidden sm:inline">{formatBitrate(camera.bitrate_kbps)}</span>
        </div>
        <IconButton label={t('Plein écran', 'Full screen')} icon={<Maximize className="size-4" />} disabledReason={reason} onClick={() => void ref.current?.requestFullscreen().catch(() => undefined)} />
      </footer>
    </article>
  )
})

/** Cameras per row: `auto` follows the screen width; a number is a maximum (small screens show fewer). */
export type GridColumns = 'auto' | 1 | 2 | 3 | 4

const GRID_CLASS: Record<GridColumns, string> = {
  auto: 'md:grid-cols-2 min-[1800px]:grid-cols-3 min-[2400px]:grid-cols-4',
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
    <div className={`grid grid-cols-1 gap-4 ${GRID_CLASS[columns]}`}>
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
