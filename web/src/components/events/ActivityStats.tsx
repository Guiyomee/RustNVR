import { BarChart3 } from 'lucide-react'
import { useState } from 'react'
import { getCrossings, getEventStats } from '../../api/endpoints'
import { useAsync } from '../../hooks/useAsync'
import { formatNumber } from '../../lib/format'
import { objectLabel } from '../../lib/objects'
import type { CrossingStats, EventStats } from '../../types/api'
import { Card, CardHeader } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { Segmented } from '../ui/Tabs'
import { locale, t } from '../../lib/i18n'

const PERIODS = [
  { value: 7, label: t('7 jours', '7 days') },
  { value: 30, label: t('30 jours', '30 days') },
] as const
type Days = (typeof PERIODS)[number]['value']

/** Rows shown in the camera and object lists; the rest folds into « Autres ». */
const TOP = 8

const dayFmt = new Intl.DateTimeFormat(locale(), { weekday: 'short', day: 'numeric' })
const fullDayFmt = new Intl.DateTimeFormat(locale(), { weekday: 'long', day: 'numeric', month: 'long' })
const parseDay = (key: string) => {
  const [y, m, d] = key.split('-').map(Number)
  return new Date(y ?? 1970, (m ?? 1) - 1, d ?? 1)
}

/**
 * Columns from one baseline: ≤ 24 px wide, 4 px rounded top, 2 px gap, the
 * largest one labelled, a tooltip on hover (the whole column is the target).
 */
function Columns({ values, label, tick, tip }: { values: number[]; label: string; tick: (i: number) => string | null; tip: (i: number) => string }) {
  const [hover, setHover] = useState<number | null>(null)
  const max = Math.max(1, ...values)
  const top = values.indexOf(Math.max(...values))
  return (
    <figure className="min-w-0">
      <figcaption className="mb-2 text-xs font-medium text-muted">{label}</figcaption>
      <div className="relative">
        <div className="flex h-32 items-end gap-0.5 border-b border-line" role="img" aria-label={label}>
          {values.map((v, i) => (
            <div
              key={i}
              className="relative flex h-full min-w-0 flex-1 cursor-default items-end justify-center"
              onPointerEnter={() => setHover(i)}
              onPointerLeave={() => setHover((h) => (h === i ? null : h))}
            >
              {i === top && v > 0 && <span className="absolute text-[10px] text-fg tabular" style={{ bottom: `calc(${(v / max) * 100}% + 2px)` }}>{formatNumber(v)}</span>}
              <div className={`w-full max-w-6 rounded-t ${hover === i ? 'bg-accent' : 'bg-accent/85'}`} style={{ height: v > 0 ? `max(2px, ${(v / max) * 100}%)` : 0 }} />
            </div>
          ))}
        </div>
        {hover !== null && (
          <div
            className="pointer-events-none absolute -top-2 z-10 -translate-x-1/2 -translate-y-full rounded-md border border-line bg-bg px-2 py-1 text-xs whitespace-nowrap shadow-lg"
            style={{ left: `${((hover + 0.5) / values.length) * 100}%` }}
          >
            {tip(hover)} : <span className="font-semibold tabular">{formatNumber(values[hover] ?? 0)}</span>
          </div>
        )}
        <div className="mt-1 flex gap-0.5 text-[10px] text-muted">
          {values.map((_, i) => (
            <span key={i} className="min-w-0 flex-1 overflow-visible text-center whitespace-nowrap">
              {tick(i) ?? ''}
            </span>
          ))}
        </div>
      </div>
    </figure>
  )
}

/** Horizontal bars for a ranking (cameras, objects), with the value at the end. */
function Ranking({ rows, label, empty }: { rows: { name: string; count: number }[]; label: string; empty: string }) {
  const max = Math.max(1, ...rows.map((r) => r.count))
  return (
    <figure className="min-w-0">
      <figcaption className="mb-2 text-xs font-medium text-muted">{label}</figcaption>
      {rows.length === 0 ? (
        <p className="text-sm text-muted">{empty}</p>
      ) : (
        <ul className="space-y-1.5" aria-label={label}>
          {rows.map((r) => (
            <li key={r.name} className="grid grid-cols-[7rem_minmax(0,1fr)_3rem] items-center gap-2 text-xs" title={`${r.name} : ${formatNumber(r.count)}`}>
              <span className="truncate">{r.name}</span>
              <span className="h-3">
                <span className="block h-full rounded-r bg-accent/85" style={{ width: `max(2px, ${(r.count / max) * 100}%)` }} />
              </span>
              <span className="text-right tabular">{formatNumber(r.count)}</span>
            </li>
          ))}
        </ul>
      )}
    </figure>
  )
}

function fold(rows: { name: string; count: number }[]): { name: string; count: number }[] {
  if (rows.length <= TOP) return rows
  const rest = rows.slice(TOP - 1).reduce((s, r) => s + r.count, 0)
  return [...rows.slice(0, TOP - 1), { name: t('Autres', 'Others'), count: rest }]
}

/** Événements → Activité: events by day, by hour, by camera and by object. */
export function ActivityStats({ names }: { names: (id: string) => string }) {
  const [days, setDays] = useState<Days>(7)
  const stats = useAsync(() => getEventStats(days), [days])
  const crossings = useAsync(() => getCrossings(days), [days])
  const lines: CrossingStats['lines'] = crossings.data?.lines ?? []
  const s: EventStats | null = stats.data
  return (
    <Card className="mb-4">
      <CardHeader
        title={t('Activité', 'Activity')}
        subtitle={t('Événements détectés, à l’heure du serveur', 'Detected events, in server time')}
        icon={<BarChart3 className="size-4" />}
        actions={<Segmented label={t('Période', 'Period')} options={[...PERIODS]} value={days} onChange={setDays} />}
      />
      {stats.error && !s ? (
        <ErrorState compact error={stats.error} onRetry={stats.reload} />
      ) : !s ? (
        <LoadingState compact />
      ) : (
        <div className="space-y-6 px-4 py-4">
          <div className="flex flex-wrap gap-x-8 gap-y-2 text-sm">
            <span>
              <span className="text-2xl font-semibold">{formatNumber(s.total)}</span> <span className="text-muted">{t('événements en ', 'events in ')}{days}{t(' jours', ' days')}</span>
            </span>
            <span className="self-end text-muted">
              {t('soit ', 'that is ')}{formatNumber(s.total / days, 1)}{t(' par jour · ', ' per day · ')}{formatNumber(s.total - s.without_object)}{t(' avec un objet reconnu', ' with a recognized object')}
            </span>
          </div>
          <div className="grid gap-6 lg:grid-cols-2">
            <Columns
              label={t('Par jour', 'Per day')}
              values={s.by_day.map((d) => d.count)}
              tick={(i) => (days === 7 || i % 5 === 0 || i === s.by_day.length - 1 ? dayFmt.format(parseDay(s.by_day[i]?.key ?? '')) : null)}
              tip={(i) => fullDayFmt.format(parseDay(s.by_day[i]?.key ?? ''))}
            />
            <Columns label={t('Par heure de la journée', 'Per hour of the day')} values={s.by_hour} tick={(i) => (i % 6 === 0 ? `${i} h` : null)} tip={(i) => `${i} h – ${i + 1} h`} />
            <Ranking label={t('Par caméra', 'Per camera')} rows={fold(s.by_camera.map((c) => ({ name: names(c.key), count: c.count })))} empty={t('Aucun événement.', 'No event.')} />
            <Ranking
              label={t('Par objet reconnu (un événement compte une fois par objet)', 'Per recognized object (an event counts once per object)')}
              rows={fold(s.by_object.map((o) => ({ name: objectLabel(o.key), count: o.count })))}
              empty={t('Aucun objet reconnu sur la période (IA optionnelle désactivée, ou rien de trouvé).', 'No object recognized in the period (optional AI off, or nothing found).')}
            />
          </div>
          {lines.length > 0 && (
            <figure>
              <figcaption className="mb-2 text-xs font-medium text-muted">{t('Passages des lignes', 'Line crossings')}</figcaption>
              <table className="w-full max-w-2xl text-left text-sm tabular">
                <thead className="text-xs text-muted">
                  <tr>
                    <th className="py-1 pr-4 font-normal">{t('Caméra · ligne', 'Camera · line')}</th>
                    <th className="py-1 pr-4 text-right font-normal">{t('Aujourd’hui', 'Today')}</th>
                    <th className="py-1 pr-4 text-right font-normal">{t('Sens de la flèche', 'Arrow direction')}</th>
                    <th className="py-1 pr-4 text-right font-normal">{t('Sens inverse', 'Opposite direction')}</th>
                    <th className="py-1 text-right font-normal">{t('Total ', 'Total ')}{days}{t(' j', ' d')}</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {lines.map((l) => (
                    <tr key={`${l.camera}/${l.line}`}>
                      <td className="py-1 pr-4">
                        {names(l.camera)} · {l.line}
                      </td>
                      <td className="py-1 pr-4 text-right font-semibold">{formatNumber(l.today)}</td>
                      <td className="py-1 pr-4 text-right">{formatNumber(l.forward)}</td>
                      <td className="py-1 pr-4 text-right">{formatNumber(l.backward)}</td>
                      <td className="py-1 text-right">{formatNumber(l.total)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </figure>
          )}
          <details className="text-xs">
            <summary className="cursor-pointer text-muted">{t('Voir les chiffres en tableau', 'Show the figures as a table')}</summary>
            <div className="mt-2 grid gap-4 sm:grid-cols-2">
              <table className="text-left tabular">
                <thead className="text-muted">
                  <tr>
                    <th className="pr-4 font-normal">{t('Jour', 'Day')}</th>
                    <th className="font-normal">{t('Événements', 'Events')}</th>
                  </tr>
                </thead>
                <tbody>
                  {s.by_day.map((d) => (
                    <tr key={d.key}>
                      <td className="pr-4">{fullDayFmt.format(parseDay(d.key))}</td>
                      <td>{d.count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <table className="text-left tabular">
                <thead className="text-muted">
                  <tr>
                    <th className="pr-4 font-normal">{t('Heure', 'Hour')}</th>
                    <th className="font-normal">{t('Événements', 'Events')}</th>
                  </tr>
                </thead>
                <tbody>
                  {s.by_hour.map((c, h) => (
                    <tr key={h}>
                      <td className="pr-4">
                        {h} h – {h + 1} h
                      </td>
                      <td>{c}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </details>
        </div>
      )}
    </Card>
  )
}
