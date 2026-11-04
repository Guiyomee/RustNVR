import type { ReactNode } from 'react'
import { Badge } from '../ui/Card'
import { t } from '../../lib/i18n'

/** A settings category: title, description and rows. */
export function SettingsSection({ title, description, available, children }: { title: string; description?: ReactNode; available?: boolean | null; children: ReactNode }) {
  return (
    <section className="card">
      <header className="flex flex-wrap items-start justify-between gap-2 border-b border-line px-5 py-4">
        <div>
          <h2 className="font-semibold">{title}</h2>
          {description && <p className="mt-1 text-sm text-muted">{description}</p>}
        </div>
        {available === false && <Badge>{t('Non disponible', 'Not available')}</Badge>}
        {available === true && <Badge tone="ok">{t('Actif', 'Active')}</Badge>}
      </header>
      <div className="divide-y divide-line">{children}</div>
    </section>
  )
}

/** One setting: label + help on the left, value or control on the right. */
export function SettingRow({ label, help, children, advanced = false }: { label: string; help?: ReactNode; children: ReactNode; advanced?: boolean }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 px-5 py-3.5">
      <div className="min-w-0 max-w-xl">
        <div className="flex items-center gap-2 text-sm font-medium">
          {label}
          {advanced && <Badge>{t('Avancé', 'Advanced')}</Badge>}
        </div>
        {help && <div className="mt-0.5 text-xs text-muted">{help}</div>}
      </div>
      <div className="text-sm">{children}</div>
    </div>
  )
}

/** Where a value is configured today (file-based configuration). */
export function ConfigKey({ name }: { name: string }) {
  return <code className="rounded bg-bg-2 px-1.5 py-0.5 text-xs text-muted">{name}</code>
}
