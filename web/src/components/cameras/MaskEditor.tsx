import { Trash } from 'lucide-react'
import { useRef, useState, type PointerEvent, type ReactNode } from 'react'
import type { PrivacyMask } from '../../types/api'
import { Button } from '../ui/Button'
import { t } from '../../lib/i18n'

const MAX_MASKS = 8
/** Smallest mask kept (fraction of the picture side): avoids stray clicks. */
const MIN_SIDE = 0.02

/**
 * Privacy masks over a camera picture: drag to draw a rectangle. Coordinates
 * are fractions of the picture (0..1).
 */
export function MaskEditor({ background, masks, onChange, disabled = false }: { background: ReactNode; masks: PrivacyMask[]; onChange: (masks: PrivacyMask[]) => void; disabled?: boolean }) {
  const svgRef = useRef<SVGSVGElement>(null)
  const [start, setStart] = useState<[number, number] | null>(null)
  const [end, setEnd] = useState<[number, number] | null>(null)
  const toNorm = (e: PointerEvent): [number, number] => {
    const r = svgRef.current?.getBoundingClientRect()
    if (!r) return [0, 0]
    const clamp = (v: number) => Math.round(Math.max(0, Math.min(1, v)) * 1000) / 1000
    return [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)]
  }
  const rectOf = (a: [number, number], b: [number, number]): [number, number, number, number] => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])]
  const preview = start && end ? rectOf(start, end) : null
  const full = masks.length >= MAX_MASKS
  const W = 1000
  const H = 562.5

  return (
    <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_280px]">
      <div className="relative aspect-video overflow-hidden rounded-md border border-line bg-bg-2">
        <div className="absolute inset-0">{background}</div>
        <svg
          ref={svgRef}
          viewBox={`0 0 ${W} ${H}`}
          preserveAspectRatio="none"
          className={`absolute inset-0 h-full w-full touch-none ${!disabled && !full ? 'cursor-crosshair' : ''}`}
          role="application"
          aria-label={t('Éditeur de masques : faites glisser pour dessiner un rectangle masqué', 'Mask editor: drag to draw a hidden rectangle')}
          onPointerDown={(e) => {
            if (disabled || full) return
            e.currentTarget.setPointerCapture(e.pointerId)
            const p = toNorm(e)
            setStart(p)
            setEnd(p)
          }}
          onPointerMove={(e) => start && setEnd(toNorm(e))}
          onPointerUp={(e) => {
            if (!start) return
            const r = rectOf(start, toNorm(e))
            setStart(null)
            setEnd(null)
            if (r[2] - r[0] >= MIN_SIDE && r[3] - r[1] >= MIN_SIDE) onChange([...masks, { name: t(`Masque ${masks.length + 1}`, `Mask ${masks.length + 1}`), rect: r }])
          }}
        >
          {masks.map((m, i) => (
            <g key={i}>
              <rect x={m.rect[0] * W} y={m.rect[1] * H} width={(m.rect[2] - m.rect[0]) * W} height={(m.rect[3] - m.rect[1]) * H} fill="black" fillOpacity={0.85} stroke="#f59e0b" strokeWidth={2} />
              <text x={m.rect[0] * W + 6} y={m.rect[1] * H + 18} fill="#f9fafb" fontSize={14}>
                {m.name}
              </text>
            </g>
          ))}
          {preview && <rect x={preview[0] * W} y={preview[1] * H} width={(preview[2] - preview[0]) * W} height={(preview[3] - preview[1]) * H} fill="black" fillOpacity={0.5} stroke="#f59e0b" strokeWidth={2} strokeDasharray="6 4" />}
        </svg>
      </div>
      <div className="space-y-2 text-sm">
        {masks.length === 0 ? (
          <p className="text-xs text-muted">{t("Aucun masque. Faites glisser sur l’image pour cacher une zone (fenêtre d’un voisin, rue…).", "No mask. Drag on the picture to hide an area (a neighbour's window, the street…).")}</p>
        ) : (
          masks.map((m, i) => (
            <div key={i} className="flex items-center gap-2">
              <input
                className="h-8 min-w-0 flex-1 rounded-md border border-line bg-bg-2 px-2 text-sm disabled:opacity-60"
                value={m.name}
                maxLength={64}
                disabled={disabled}
                aria-label={t('Nom du masque', 'Mask name')}
                onChange={(e) => onChange(masks.map((x, k) => (k === i ? { ...x, name: e.target.value } : x)))}
              />
              <Button size="sm" icon={<Trash className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} aria-label={t(`Supprimer ${m.name}`, `Delete ${m.name}`)} onClick={() => onChange(masks.filter((_, k) => k !== i))} />
            </div>
          ))
        )}
        <p className="text-xs text-muted">
          {t('Zones noircies dans le direct, la relecture, les vignettes, les captures et les exports (un export d’une caméra masquée est converti en H.264), et ignorées par la détection. Les enregistrements gardent l’image complète (pas de réencodage) : le direct et la relecture sont masqués dans l’interface.', 'Areas blacked out in the live view, playback, thumbnails, snapshots and exports (an export of a masked camera is converted to H.264), and ignored by detection. Recordings keep the full picture (no re-encoding): the live view and playback are masked in the interface.')}
        </p>
      </div>
    </div>
  )
}
