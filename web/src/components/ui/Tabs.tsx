import type { ReactNode } from 'react'

export interface TabDef<K extends string> {
  key: K
  label: string
  icon?: ReactNode
}

export function Tabs<K extends string>({ tabs, active, onChange }: { tabs: TabDef<K>[]; active: K; onChange: (k: K) => void }) {
  return (
    <div role="tablist" className="flex gap-1 overflow-x-auto border-b border-line">
      {tabs.map((t) => {
        const selected = t.key === active
        return (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={selected}
            onClick={() => onChange(t.key)}
            className={`-mb-px inline-flex shrink-0 items-center gap-2 border-b-2 px-3 py-2.5 text-sm transition-colors ${
              selected ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'
            }`}
          >
            {t.icon}
            {t.label}
          </button>
        )
      })}
    </div>
  )
}

/** Segmented control (e.g. timeline window 1h/6h/12h/24h). */
export function Segmented<K extends string | number>({ options, value, onChange, label }: { options: { value: K; label: string }[]; value: K; onChange: (v: K) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex rounded-md border border-line bg-bg-2 p-0.5">
      {options.map((o) => (
        <button
          key={String(o.value)}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          onClick={() => onChange(o.value)}
          className={`rounded px-2.5 py-1 text-xs font-medium tabular transition-colors ${o.value === value ? 'bg-accent text-white' : 'text-muted hover:text-fg'}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}
