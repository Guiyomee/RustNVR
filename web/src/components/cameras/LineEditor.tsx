import { Trash } from 'lucide-react'
import { useRef, useState, type PointerEvent, type ReactNode } from 'react'
import type { CrossingLine } from '../../types/api'
import { Button } from '../ui/Button'
import { t } from '../../lib/i18n'

const MAX_LINES = 4
const W = 1000
const H = 562.5

const DIRECTIONS: { value: CrossingLine['direction']; label: string }[] = [
  { value: 'both', label: t('Les deux sens', 'Both ways') },
  { value: 'forward', label: t('Sens de la flèche', 'Arrow direction') },
  { value: 'backward', label: t('Sens inverse', 'Opposite direction') },
]

/** Arrow showing the « forward » side: from the left of a → b to its right. */
function Arrow({ l }: { l: CrossingLine }) {
  const [ax, ay, bx, by] = [l.a[0] * W, l.a[1] * H, l.b[0] * W, l.b[1] * H]
  const [mx, my] = [(ax + bx) / 2, (ay + by) / 2]
  const len = Math.hypot(bx - ax, by - ay) || 1
  // Right of a → b with y downwards: (-dy, dx).
  const [nx, ny] = [-(by - ay) / len, (bx - ax) / len]
  const tip: [number, number] = [mx + nx * 40, my + ny * 40]
  const head = (s: number): [number, number] => [tip[0] - nx * 12 + s * ny * 8, tip[1] - ny * 12 - s * nx * 8]
  const [h1, h2] = [head(1), head(-1)]
  const show = l.direction !== 'backward'
  const back = l.direction !== 'forward'
  return (
    <g stroke="#f9fafb" strokeWidth={3} fill="none">
      {show && <path d={`M ${mx} ${my} L ${tip[0]} ${tip[1]} M ${h1[0]} ${h1[1]} L ${tip[0]} ${tip[1]} L ${h2[0]} ${h2[1]}`} />}
      {back && <path d={`M ${mx} ${my} L ${mx - nx * 40} ${my - ny * 40}`} strokeDasharray={l.direction === 'both' ? '0' : '6 4'} />}
    </g>
  )
}

/**
 * Crossing lines over a camera picture: click two points to draw a line, then
 * choose which crossings count. Coordinates are fractions of the picture.
 */
export function LineEditor({ background, lines, onChange, disabled = false }: { background: ReactNode; lines: CrossingLine[]; onChange: (lines: CrossingLine[]) => void; disabled?: boolean }) {
  const svgRef = useRef<SVGSVGElement>(null)
  const [first, setFirst] = useState<[number, number] | null>(null)
  const [hover, setHover] = useState<[number, number] | null>(null)
  const toNorm = (e: PointerEvent): [number, number] => {
    const r = svgRef.current?.getBoundingClientRect()
    if (!r) return [0, 0]
    const clamp = (v: number) => Math.round(Math.max(0, Math.min(1, v)) * 1000) / 1000
    return [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)]
  }
  const full = lines.length >= MAX_LINES
  const set = (i: number, patch: Partial<CrossingLine>) => onChange(lines.map((l, k) => (k === i ? { ...l, ...patch } : l)))

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
          aria-label={t('Éditeur de lignes de passage : cliquez deux points pour tracer une ligne', 'Crossing line editor: click two points to draw a line')}
          onPointerDown={(e) => {
            if (disabled || full) return
            const p = toNorm(e)
            if (!first) {
              setFirst(p)
              return
            }
            setFirst(null)
            if (Math.hypot(p[0] - first[0], p[1] - first[1]) >= 0.02) onChange([...lines, { name: t(`Ligne ${lines.length + 1}`, `Line ${lines.length + 1}`), a: first, b: p, direction: 'both' }])
          }}
          onPointerMove={(e) => first && setHover(toNorm(e))}
        >
          {lines.map((l, i) => (
            <g key={i}>
              <line x1={l.a[0] * W} y1={l.a[1] * H} x2={l.b[0] * W} y2={l.b[1] * H} stroke="#22c55e" strokeWidth={4} strokeLinecap="round" />
              <Arrow l={l} />
              <text x={l.a[0] * W + 6} y={l.a[1] * H - 8} fill="#f9fafb" fontSize={16} stroke="#000" strokeWidth={0.6}>
                {l.name}
              </text>
            </g>
          ))}
          {first && <circle cx={first[0] * W} cy={first[1] * H} r={6} fill="#22c55e" />}
          {first && hover && <line x1={first[0] * W} y1={first[1] * H} x2={hover[0] * W} y2={hover[1] * H} stroke="#22c55e" strokeWidth={3} strokeDasharray="8 6" />}
        </svg>
      </div>
      <div className="space-y-3 text-sm">
        {lines.length === 0 ? (
          <p className="text-xs text-muted">{t('Aucune ligne. Cliquez deux points sur l’image (portail, porte, allée) : chaque passage de ce qui bouge par-dessus sera compté.', 'No line. Click two points on the picture (gate, door, path): each crossing of what moves over it will be counted.')}</p>
        ) : (
          lines.map((l, i) => (
            <div key={i} className="space-y-1.5">
              <div className="flex items-center gap-2">
                <input
                  className="h-8 min-w-0 flex-1 rounded-md border border-line bg-bg-2 px-2 text-sm disabled:opacity-60"
                  value={l.name}
                  maxLength={64}
                  disabled={disabled}
                  aria-label={t('Nom de la ligne', 'Line name')}
                  onChange={(e) => set(i, { name: e.target.value })}
                />
                <Button size="sm" icon={<Trash className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} aria-label={t(`Supprimer ${l.name}`, `Delete ${l.name}`)} onClick={() => onChange(lines.filter((_, k) => k !== i))} />
              </div>
              <select className="h-8 w-full rounded-md border border-line bg-bg-2 px-2 text-sm" value={l.direction} disabled={disabled} aria-label={t('Passages comptés', 'Crossings counted')} onChange={(e) => set(i, { direction: e.target.value as CrossingLine['direction'] })}>
                {DIRECTIONS.map((d) => (
                  <option key={d.value} value={d.value}>
                    {d.label}
                  </option>
                ))}
              </select>
            </div>
          ))
        )}
        <p className="text-xs text-muted">
          {t('Compte le passage du centre de ce qui bouge au-dessus de la ligne (5 images par seconde, sans IA) ; nécessite la détection activée. Fiable pour une personne ou un véhicule à la fois (portail, porte) ; plusieurs mouvements simultanés se confondent. Compteurs : Événements → Activité, et MQTT.', 'Counts when the centre of what moves passes over the line (5 pictures per second, no AI); needs detection on. Reliable for one person or vehicle at a time (gate, door); several simultaneous movements get mixed up. Counters: Events → Activity, and MQTT.')}
        </p>
      </div>
    </div>
  )
}
