import { memo, useState } from 'react'

interface Props {
  values: number[]
  /** Fixed domain (e.g. 0-100 for percentages); defaults to the data range. */
  min?: number
  max?: number
  label: string
  format?: (v: number) => string
  width?: number
  height?: number
}

/**
 * Trend line for a stat tile: de-emphasized line, current value in the
 * accent color, value under the pointer on hover.
 */
export const Sparkline = memo(function Sparkline({ values, min, max, label, format = String, width = 120, height = 32 }: Props) {
  const [hover, setHover] = useState<number | null>(null)
  if (values.length < 2) {
    return <div style={{ width, height }} aria-hidden />
  }
  const lo = min ?? Math.min(...values)
  const hi = max ?? Math.max(...values)
  const span = hi - lo || 1
  const pad = 3
  const x = (i: number) => pad + (i * (width - 2 * pad)) / (values.length - 1)
  const y = (v: number) => height - pad - ((v - lo) / span) * (height - 2 * pad)
  const points = values.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(' ')
  const last = values.length - 1
  const shown = hover ?? last
  const shownValue = values[shown] ?? 0

  return (
    <div className="relative" style={{ width, height }}>
      <svg
        width={width}
        height={height}
        role="img"
        aria-label={`${label} : ${format(values[last] ?? 0)}`}
        onMouseLeave={() => setHover(null)}
        onMouseMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          const i = Math.round(((e.clientX - r.left - pad) / (width - 2 * pad)) * last)
          setHover(Math.max(0, Math.min(last, i)))
        }}
      >
        <polyline points={points} fill="none" stroke="var(--color-muted)" strokeOpacity={0.55} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
        {hover !== null && <line x1={x(shown)} x2={x(shown)} y1={0} y2={height} stroke="var(--color-line)" strokeWidth={1} />}
        <circle cx={x(shown)} cy={y(shownValue)} r={3.5} fill="var(--color-accent)" stroke="var(--color-card)" strokeWidth={2} />
      </svg>
      {hover !== null && (
        <div className="pointer-events-none absolute -top-7 right-0 rounded border border-line bg-bg-2 px-1.5 py-0.5 text-[11px] whitespace-nowrap tabular">
          {format(shownValue)}
        </div>
      )}
    </div>
  )
})
