import { Plus, Trash2 } from 'lucide-react'
import type { TimeRange } from '../../types/api'
import { Button, IconButton } from './Button'
import { t } from '../../lib/i18n'

/** Most ranges in one schedule (as the server). */
const MAX_RANGES = 16

const DAYS: { day: number; short: [string, string]; name: [string, string] }[] = [
  { day: 1, short: ['L', 'M'], name: ['lundi', 'Monday'] },
  { day: 2, short: ['M', 'T'], name: ['mardi', 'Tuesday'] },
  { day: 3, short: ['M', 'W'], name: ['mercredi', 'Wednesday'] },
  { day: 4, short: ['J', 'T'], name: ['jeudi', 'Thursday'] },
  { day: 5, short: ['V', 'F'], name: ['vendredi', 'Friday'] },
  { day: 6, short: ['S', 'S'], name: ['samedi', 'Saturday'] },
  { day: 7, short: ['D', 'S'], name: ['dimanche', 'Sunday'] },
]

/**
 * Weekly time ranges (local time of the server). Without any range, the
 * thing is active all the time; `what` says what the ranges allow.
 */
export function ScheduleEditor({ value, onChange, disabled, what }: { value: TimeRange[]; onChange: (v: TimeRange[]) => void; disabled?: boolean; what: string }) {
  const set = (i: number, patch: Partial<TimeRange>) => onChange(value.map((r, j) => (j === i ? { ...r, ...patch } : r)))
  const toggleDay = (i: number, day: number) => {
    const r = value[i]
    if (!r) return
    const days = r.days.length === 0 ? DAYS.map((d) => d.day) : r.days
    const next = days.includes(day) ? days.filter((d) => d !== day) : [...days, day].sort((a, b) => a - b)
    set(i, { days: next.length === 7 ? [] : next })
  }
  const input = 'h-8 rounded-md border border-line bg-bg-2 px-2 text-sm tabular disabled:opacity-60'
  return (
    <div className="space-y-2 text-sm">
      {value.length === 0 ? (
        <p className="text-xs text-muted">{t(`Aucune plage : ${what} tout le temps.`, `No range: ${what} all the time.`)}</p>
      ) : (
        <ul className="space-y-2">
          {value.map((r, i) => (
            <li key={i} className="flex flex-wrap items-center gap-2 rounded-md border border-line p-2">
              <div className="flex gap-1" role="group" aria-label={t('Jours', 'Days')}>
                {DAYS.map((d) => {
                  const on = r.days.length === 0 || r.days.includes(d.day)
                  return (
                    <button
                      key={d.day}
                      type="button"
                      disabled={disabled}
                      aria-pressed={on}
                      title={t(d.name[0], d.name[1])}
                      onClick={() => toggleDay(i, d.day)}
                      className={`size-7 rounded-full border text-xs font-medium disabled:opacity-60 ${on ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted hover:text-fg'}`}
                    >
                      {t(d.short[0], d.short[1])}
                    </button>
                  )
                })}
              </div>
              <label className="flex items-center gap-1">
                <span className="text-xs text-muted">{t('de', 'from')}</span>
                <input type="time" className={input} value={r.from} disabled={disabled} onChange={(e) => set(i, { from: e.target.value })} aria-label={t('Début', 'Start')} />
              </label>
              <label className="flex items-center gap-1">
                <span className="text-xs text-muted">{t('à', 'to')}</span>
                <input type="time" className={input} value={r.to} disabled={disabled} onChange={(e) => set(i, { to: e.target.value })} aria-label={t('Fin', 'End')} />
              </label>
              <span className="text-xs text-muted">{r.from === r.to ? t('toute la journée', 'all day') : r.to < r.from ? t('jusqu’au lendemain', 'until the next day') : ''}</span>
              <IconButton className="ml-auto" label={t('Supprimer la plage', 'Remove the range')} icon={<Trash2 className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} onClick={() => onChange(value.filter((_, j) => j !== i))} />
            </li>
          ))}
        </ul>
      )}
      <Button
        size="sm"
        icon={<Plus className="size-3.5" />}
        disabledReason={disabled ? t('Lecture seule', 'Read only') : value.length >= MAX_RANGES ? t(`${MAX_RANGES} plages au plus`, `${MAX_RANGES} ranges at most`) : null}
        onClick={() => onChange([...value, { days: [], from: '22:00', to: '06:00' }])}
      >
        {t('Ajouter une plage', 'Add a range')}
      </Button>
    </div>
  )
}
