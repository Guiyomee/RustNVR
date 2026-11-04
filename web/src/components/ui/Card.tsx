import type { ReactNode } from 'react'

export function Card({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <section className={`card ${className}`}>{children}</section>
}

export function CardHeader({ title, icon, actions, subtitle }: { title: string; icon?: ReactNode; actions?: ReactNode; subtitle?: ReactNode }) {
  return (
    <header className="flex items-center justify-between gap-3 border-b border-line px-4 py-3">
      <div className="flex min-w-0 items-center gap-2">
        {icon && <span className="text-muted">{icon}</span>}
        <div className="min-w-0">
          <h2 className="truncate text-sm font-semibold">{title}</h2>
          {subtitle && <p className="truncate text-xs text-muted">{subtitle}</p>}
        </div>
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1">{actions}</div>}
    </header>
  )
}

export function PageHeader({ title, description, actions }: { title: string; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-5 flex flex-wrap items-end justify-between gap-3">
      <div>
        <h1 className="text-xl font-semibold">{title}</h1>
        {description && <p className="mt-1 text-sm text-muted">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  )
}

export function Badge({ children, tone = 'neutral' }: { children: ReactNode; tone?: 'neutral' | 'ok' | 'warn' | 'err' | 'info' | 'demo' }) {
  const tones = {
    neutral: 'bg-line/70 text-muted',
    ok: 'bg-ok/15 text-ok',
    warn: 'bg-warn/15 text-warn',
    err: 'bg-err/15 text-err',
    info: 'bg-accent/15 text-accent',
    demo: 'bg-warn text-bg-2',
  }
  return <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-semibold uppercase tracking-wide ${tones[tone]}`}>{children}</span>
}

/** Key/value rows for technical details. */
export function InfoList({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="divide-y divide-line">
      {items.map(([k, v]) => (
        <div key={k} className="flex items-baseline justify-between gap-4 py-2 text-sm">
          <dt className="text-muted">{k}</dt>
          <dd className="tabular min-w-0 truncate text-right">{v}</dd>
        </div>
      ))}
    </dl>
  )
}
