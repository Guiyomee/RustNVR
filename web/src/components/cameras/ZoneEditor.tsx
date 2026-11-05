import { Plus, Trash, Undo2 } from 'lucide-react'
import { useRef, useState, type PointerEvent, type ReactNode } from 'react'
import { objectLabel } from '../../lib/objects'
import type { MotionZone } from '../../types/api'
import { Button } from '../ui/Button'
import { t } from '../../lib/i18n'

const COLORS = ['#3b82f6', '#d97706', '#22c55e', '#a855f7']
const MAX_ZONES = 8
const MAX_POINTS = 32

/**
 * Polygon editor over a camera picture. Click to add a point to the selected
 * zone, drag a point to move it. Coordinates are fractions of the picture
 * (0..1) so that zones survive resolution changes. Without any zone, the
 * whole picture is analysed.
 */
export function ZoneEditor({
  background,
  zones,
  onChange,
  disabled = false,
  objects = [],
}: {
  background: ReactNode
  zones: MotionZone[]
  onChange: (zones: MotionZone[]) => void
  disabled?: boolean
  /** Recognizable objects (optional AI) that can be restricted to a zone; empty: none. */
  objects?: string[]
}) {
  const [selected, setSelected] = useState(0)
  const [drag, setDrag] = useState<{ zone: number; point: number } | null>(null)
  const svgRef = useRef<SVGSVGElement>(null)

  const toNorm = (e: PointerEvent): [number, number] => {
    const r = svgRef.current?.getBoundingClientRect()
    if (!r) return [0, 0]
    const clamp = (v: number) => Math.round(Math.max(0, Math.min(1, v)) * 1000) / 1000
    return [clamp((e.clientX - r.left) / r.width), clamp((e.clientY - r.top) / r.height)]
  }
  const update = (i: number, fn: (z: MotionZone) => MotionZone) => onChange(zones.map((z, k) => (k === i ? fn(z) : z)))
  const current = zones[selected] as MotionZone | undefined

  return (
    <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_280px]">
      <div className="relative aspect-video overflow-hidden rounded-md border border-line bg-bg-2">
        <div className="absolute inset-0">{background}</div>
        <svg
          ref={svgRef}
          viewBox="0 0 1000 562.5"
          preserveAspectRatio="none"
          className={`absolute inset-0 h-full w-full touch-none ${current && !disabled ? 'cursor-crosshair' : ''}`}
          role="application"
          aria-label={t('Éditeur de zones : cliquez pour ajouter un point à la zone sélectionnée', 'Zone editor: click to add a point to the selected zone')}
          onPointerDown={(e) => {
            if (disabled || drag || !current || e.target !== e.currentTarget || current.points.length >= MAX_POINTS) return
            const p = toNorm(e)
            update(selected, (z) => ({ ...z, points: [...z.points, p] }))
          }}
          onPointerMove={(e) => {
            if (!drag) return
            const p = toNorm(e)
            update(drag.zone, (z) => ({ ...z, points: z.points.map((q, k) => (k === drag.point ? p : q)) }))
          }}
          onPointerUp={() => setDrag(null)}
          onPointerLeave={() => setDrag(null)}
        >
          {zones.map((z, i) => {
            const color = COLORS[i % COLORS.length]
            const pts = z.points.map(([x, y]) => `${x * 1000},${y * 562.5}`).join(' ')
            return (
              <g key={i} opacity={i === selected ? 1 : 0.55}>
                {z.points.length >= 3 && <polygon points={pts} fill={color} fillOpacity={0.18} stroke={color} strokeWidth={3} />}
                {z.points.length === 2 && <polyline points={pts} fill="none" stroke={color} strokeWidth={3} />}
                {z.points.map(([x, y], k) => (
                  <circle
                    key={k}
                    cx={x * 1000}
                    cy={y * 562.5}
                    r={i === selected ? 10 : 7}
                    fill={color}
                    stroke="#0b1220"
                    strokeWidth={3}
                    className={disabled ? '' : 'cursor-move'}
                    onPointerDown={(e) => {
                      e.stopPropagation()
                      setSelected(i)
                      if (!disabled) setDrag({ zone: i, point: k })
                    }}
                  />
                ))}
              </g>
            )
          })}
        </svg>
        {zones.length === 0 && (
          <p className="pointer-events-none absolute bottom-2 left-2 rounded bg-bg/80 px-2 py-1 text-xs text-muted">{t("Aucune zone : toute l'image est analysée.", "No zone: the whole picture is analysed.")}</p>
        )}
      </div>

      <div className="space-y-3">
        <ul className="space-y-1.5">
          {zones.map((z, i) => (
            <li key={i}>
              <button
                type="button"
                onClick={() => setSelected(i)}
                className={`flex w-full items-center gap-2 rounded-md border px-3 py-2 text-left text-sm ${i === selected ? 'border-accent bg-accent/10' : 'border-line hover:bg-line/40'}`}
              >
                <span className="size-3 rounded-sm" style={{ background: COLORS[i % COLORS.length] }} aria-hidden />
                <span className="flex-1 truncate">{z.name}</span>
                <span className={`text-xs ${z.points.length < 3 ? 'text-warn' : 'text-muted'}`}>{z.points.length}{t(' pts', ' pts')}</span>
              </button>
            </li>
          ))}
        </ul>
        {current && (
          <div className="space-y-3 rounded-md border border-line p-3">
            <label className="block text-xs text-muted">
              {t('Nom', 'Name')}
              <input
                className="mt-1 h-8 w-full rounded border border-line bg-bg-2 px-2 text-sm text-fg"
                value={current.name}
                maxLength={64}
                disabled={disabled}
                onChange={(e) => update(selected, (z) => ({ ...z, name: e.target.value }))}
              />
            </label>
            <label className="block text-xs text-muted">
              {t('Sensibilité dans cette zone : ', 'Sensitivity in this zone: ')}<span className="tabular text-fg">{current.sensitivity}</span>
              <input
                type="range"
                min={1}
                max={100}
                className="mt-1 w-full accent-accent"
                value={current.sensitivity}
                disabled={disabled}
                onChange={(e) => update(selected, (z) => ({ ...z, sensitivity: Number(e.target.value) }))}
              />
            </label>
            {objects.length > 0 && (
              <div className="text-xs text-muted">
                {t('Objets qui ne comptent que dans cette zone', 'Objects that only count in this zone')}
                <div className="mt-1 flex flex-wrap gap-1.5">
                  {[...new Set([...objects, ...(current.objects ?? [])])].map((label) => {
                    const on = (current.objects ?? []).includes(label)
                    return (
                      <label key={label} className={`inline-flex cursor-pointer items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] ${on ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted'} ${disabled ? 'pointer-events-none opacity-60' : ''}`}>
                        <input
                          type="checkbox"
                          className="size-3 accent-accent"
                          checked={on}
                          disabled={disabled}
                          onChange={() =>
                            update(selected, (z) => {
                              const list = z.objects ?? []
                              return { ...z, objects: on ? list.filter((l) => l !== label) : [...list, label] }
                            })
                          }
                        />
                        {objectLabel(label)}
                      </label>
                    )
                  })}
                </div>
                <span className="mt-1 block">{t("Par exemple « Personne » pour une allée : une personne n'est alors gardée que si ses pieds sont dans une zone où elle est cochée. Les objets cochés nulle part comptent dans toutes les zones, jamais en dehors.", "For example « Person » for a path: a person is then only kept if their feet are in a zone where it is ticked. Objects ticked nowhere count in every zone, never outside.")}</span>
              </div>
            )}
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                icon={<Undo2 className="size-3.5" />}
                disabledReason={disabled ? t('Lecture seule.', 'Read only.') : current.points.length ? null : t('Aucun point à retirer.', 'No point to remove.')}
                onClick={() => update(selected, (z) => ({ ...z, points: z.points.slice(0, -1) }))}
              >
                {t('Dernier point', 'Last point')}
              </Button>
              <Button
                size="sm"
                variant="danger"
                icon={<Trash className="size-3.5" />}
                disabledReason={disabled ? t('Lecture seule.', 'Read only.') : null}
                onClick={() => {
                  onChange(zones.filter((_, k) => k !== selected))
                  setSelected(0)
                }}
              >
                {t('Supprimer', 'Delete')}
              </Button>
            </div>
          </div>
        )}
        <Button
          size="sm"
          icon={<Plus className="size-3.5" />}
          disabledReason={disabled ? t('Lecture seule.', 'Read only.') : zones.length >= MAX_ZONES ? t(`${MAX_ZONES} zones au maximum.`, `${MAX_ZONES} zones at most.`) : null}
          onClick={() => {
            onChange([...zones, { name: t(`Zone ${zones.length + 1}`, `Zone ${zones.length + 1}`), points: [], sensitivity: 50 }])
            setSelected(zones.length)
          }}
        >
          {t('Nouvelle zone', 'New zone')}
        </Button>
        <p className="text-xs text-muted">
          {t("Cliquez sur l'image pour ajouter des points (3 au minimum) ; faites glisser un point pour le déplacer. Le mouvement n'est compté que dans les zones.", "Click on the picture to add points (3 at least); drag a point to move it. Motion only counts inside the zones.")}
        </p>
      </div>
    </div>
  )
}
