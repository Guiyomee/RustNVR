import type { ReactNode } from 'react'
import { toneText, type Tone } from './StatusIndicator'

interface Props {
  label: string
  icon: ReactNode
  value: ReactNode
  /** Secondary line under the value (e.g. "sur 6 caméras"). */
  detail?: ReactNode
  /** Tooltip of the card (what the figures measure). */
  title?: string
  tone?: Tone
  /** Sparkline, meter… */
  children?: ReactNode
}

/** Compact headline figure: label, value, detail and an optional trend. */
export function StatCard({ label, icon, value, detail, title, tone = 'info', children }: Props) {
  return (
    <section className="card flex min-w-0 flex-col gap-2 p-4" title={title}>
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-xs font-medium text-muted">{label}</h2>
        <span className={`flex size-7 items-center justify-center rounded-md bg-bg-2 ${toneText[tone]}`} aria-hidden>
          {icon}
        </span>
      </div>
      <div className="flex items-end justify-between gap-3">
        <div className="min-w-0">
          <div className="tabular truncate text-2xl font-semibold leading-none">{value}</div>
          {detail && <div className="mt-1.5 truncate text-xs text-muted">{detail}</div>}
        </div>
        {children && <div className="shrink-0">{children}</div>}
      </div>
    </section>
  )
}
