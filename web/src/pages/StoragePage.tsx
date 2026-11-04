import { CalendarDays, HardDrive, Network, Video } from 'lucide-react'
import { useMemo, useState } from 'react'
import { StorageSummary } from '../components/storage/StorageCard'
import { Button } from '../components/ui/Button'
import { Badge, Card, CardHeader, PageHeader } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { useCapability, useLive } from '../data/DataContext'
import { useStorage } from '../hooks/useStorage'
import { formatBytes, formatDate, formatDateTime, formatNumber } from '../lib/format'
import { navigate } from '../lib/router'
import type { DayStorage, Destination } from '../types/api'
import { t } from '../lib/i18n'

function retentionLabel(d: Destination): string {
  const parts = [d.retention_days != null && t(`${d.retention_days} jours`, `${d.retention_days} days`), d.max_size_gb != null && t(`${d.max_size_gb} Go max.`, `${d.max_size_gb} GB max.`)].filter(Boolean)
  return parts.length > 0 ? t(`rétention ${parts.join(', ')}`, `retention ${parts.join(', ')}`) : t('conservation illimitée', 'unlimited retention')
}

/** Bytes written per day: one series, one bar per day, value on hover. */
function DailyChart({ days }: { days: DayStorage[] }) {
  const [hover, setHover] = useState<number | null>(null)
  const shown = days.slice(-31)
  const max = Math.max(1, ...shown.map((d) => d.bytes))
  if (shown.length === 0) return <EmptyState title={t('Aucun enregistrement', 'No recording')} />
  return (
    <div>
      <div className="relative flex h-44 items-end gap-0.5 border-b border-line" role="img" aria-label={t('Espace occupé par jour', 'Space used per day')}>
        {shown.map((d, i) => (
          <div
            key={d.day}
            className="group relative flex h-full max-w-12 flex-1 items-end"
            onMouseEnter={() => setHover(i)}
            onMouseLeave={() => setHover(null)}
          >
            <div className={`w-full rounded-t-[4px] ${hover === i ? 'bg-accent' : 'bg-accent/75'}`} style={{ height: `${Math.max(1, (d.bytes / max) * 100)}%` }} />
            {hover === i && (
              <div className={`pointer-events-none absolute bottom-full z-10 mb-1 rounded border border-line bg-bg px-2 py-1 text-xs whitespace-nowrap shadow ${i > shown.length / 2 ? 'right-0' : 'left-0'}`}>
                <div className="font-medium">{d.day}</div>
                <div className="tabular text-muted">{formatBytes(d.bytes)}</div>
              </div>
            )}
          </div>
        ))}
      </div>
      <div className="mt-1 flex justify-between text-[11px] text-muted tabular">
        <span>{shown[0]?.day}</span>
        <span>{t('max. ', 'max. ')}{formatBytes(max)}{t(' / jour', ' / day')}</span>
        <span>{shown[shown.length - 1]?.day}</span>
      </div>
      <details className="mt-3 text-xs">
        <summary className="cursor-pointer text-muted hover:text-fg">{t('Voir le tableau', 'Show the table')}</summary>
        <table className="mt-2 w-full">
          <tbody className="divide-y divide-line">
            {[...shown].reverse().map((d) => (
              <tr key={d.day}>
                <td className="py-1">{d.day}</td>
                <td className="py-1 text-right tabular">{formatBytes(d.bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </div>
  )
}

export function StoragePage() {
  const storage = useStorage()
  const cameras = useLive((s) => s.cameras)
  const retention = useCapability('retention')
  const names = useMemo(() => new Map(cameras.map((c) => [c.id, c.name])), [cameras])
  const s = storage.data

  return (
    <>
      <PageHeader title={t('Stockage', 'Storage')} description={s ? t(`Enregistrements dans ${s.recordings_dir}`, `Recordings in ${s.recordings_dir}`) : undefined} />
      {storage.error && !s ? (
        <Card>
          <ErrorState error={storage.error} onRetry={storage.reload} />
        </Card>
      ) : !s ? (
        <LoadingState />
      ) : (
        <div className="grid gap-4 xl:grid-cols-3">
          <Card>
            <CardHeader title={t('Disque des enregistrements', 'Recordings disk')} icon={<HardDrive className="size-4" />} subtitle={t(`Calculé le ${formatDateTime(s.computed_at_ms)}`, `Computed on ${formatDateTime(s.computed_at_ms)}`)} />
            <div className="p-4">
              <StorageSummary storage={s} />
            </div>
          </Card>

          <Card className="xl:col-span-2">
            <CardHeader title={t('Espace occupé par jour', 'Space used per day')} icon={<CalendarDays className="size-4" />} subtitle={t('Toutes caméras, jours UTC', 'All cameras, UTC days')} />
            <div className="p-4">
              <DailyChart days={s.days} />
            </div>
          </Card>

          <Card className="xl:col-span-2">
            <CardHeader title={t('Enregistrements par caméra', 'Recordings per camera')} icon={<Video className="size-4" />} />
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead className="border-b border-line text-left text-xs text-muted">
                  <tr>
                    <th className="px-4 py-2.5 font-medium">{t('Caméra', 'Camera')}</th>
                    <th className="px-4 py-2.5 text-right font-medium">{t('Taille', 'Size')}</th>
                    <th className="px-4 py-2.5 text-right font-medium">{t('Segments', 'Segments')}</th>
                    <th className="px-4 py-2.5 font-medium">{t('Plus ancien', 'Oldest')}</th>
                    <th className="px-4 py-2.5 font-medium">{t('Part', 'Share')}</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {s.cameras.map((c) => {
                    const share = s.recordings_bytes ? (100 * c.bytes) / s.recordings_bytes : 0
                    return (
                      <tr key={c.camera}>
                        <td className="px-4 py-2.5">{names.get(c.camera) ?? c.camera}</td>
                        <td className="tabular px-4 py-2.5 text-right">{formatBytes(c.bytes)}</td>
                        <td className="tabular px-4 py-2.5 text-right">{formatNumber(c.segments)}</td>
                        <td className="tabular px-4 py-2.5 text-xs text-muted">{c.oldest_ms ? formatDate(c.oldest_ms) : '—'}</td>
                        <td className="px-4 py-2.5">
                          <div className="flex items-center gap-2">
                            <div className="h-2 w-24 overflow-hidden rounded-full bg-accent/20">
                              <div className="h-full rounded-full bg-accent" style={{ width: `${share}%` }} />
                            </div>
                            <span className="tabular text-xs text-muted">{formatNumber(share)} %</span>
                          </div>
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          </Card>

          <Card>
            <CardHeader title={t('Points de stockage et rétention', 'Storage destinations and retention')} icon={<Network className="size-4" />} />
            <ul className="divide-y divide-line">
              {s.destinations.map((d) => (
                <li key={d.id} className="space-y-0.5 px-4 py-3 text-sm">
                  <div className="flex flex-wrap items-center gap-2 font-medium">
                    {d.name}
                    {d.id === s.default_destination && <Badge tone="info">{t('Par défaut', 'Default')}</Badge>}
                    {d.available ? <Badge tone="ok">{t('Disponible', 'Available')}</Badge> : <Badge tone="err">{t('Indisponible', 'Unavailable')}</Badge>}
                  </div>
                  <p className="truncate font-mono text-xs text-muted">{d.path}</p>
                  <p className="text-xs text-muted tabular">
                    {formatBytes(d.recordings_bytes)}{t(' de vidéos · ', ' of videos · ')}{retentionLabel(d)}
                  </p>
                </li>
              ))}
            </ul>
            <div className="border-t border-line p-4">
              <Button onClick={() => navigate('/settings?section=storage')} disabledReason={retention ? null : t("Ce serveur ne permet pas de modifier le stockage depuis l'interface.", "This server does not allow changing storage from the interface.")}>
                {t('Gérer dans Paramètres → Stockage', 'Manage in Settings → Storage')}
              </Button>
            </div>
          </Card>
        </div>
      )}
    </>
  )
}
