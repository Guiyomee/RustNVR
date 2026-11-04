import { ChevronLeft, ChevronRight, Clock } from 'lucide-react'
import { useState } from 'react'
import { useNow } from '../../hooks/useUi'
import { formatDate, formatTime, startOfDay } from '../../lib/format'
import { Button, IconButton } from '../ui/Button'
import { Card, CardHeader } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { Segmented } from '../ui/Tabs'
import { Timeline, type TimelineRow } from './Timeline'
import { useTimelineData } from './useTimelineData'
import { t } from '../../lib/i18n'

export type WindowHours = 1 | 6 | 12 | 24
export const WINDOW_OPTIONS: { value: WindowHours; label: string }[] = [
  { value: 1, label: '1 h' },
  { value: 6, label: '6 h' },
  { value: 12, label: '12 h' },
  { value: 24, label: '24 h' },
]
const HOUR = 3_600_000

/** Visible window: 24 h shows the calendar day; shorter windows end at `end`. */
export function computeWindow(hours: WindowHours, end: number | null, now: number): { start: number; end: number } {
  if (end === null) {
    if (hours === 24) {
      const s = startOfDay(now)
      return { start: s, end: s + 24 * HOUR }
    }
    return { start: now - hours * HOUR, end: now + (hours * HOUR) / 20 }
  }
  return { start: end - hours * HOUR, end }
}

export function TimelineCard({ rows, onSelect }: { rows: TimelineRow[]; onSelect?: (rowId: string, t: number) => void }) {
  const now = useNow(30_000)
  const [hours, setHours] = useState<WindowHours>(24)
  const [end, setEnd] = useState<number | null>(null)
  const win = computeWindow(hours, end, now)
  // While following the present, `now` advances every 30 s, which moves the
  // fetched range and therefore refreshes the data.
  const fetchEnd = end === null ? Math.min(win.end, now + 1) : win.end
  const data = useTimelineData(
    rows.map((r) => r.id),
    win.start,
    fetchEnd,
  )
  const shift = (dir: -1 | 1) => setEnd(win.end + (dir * hours * HOUR) / 2)
  const atPresent = end === null || win.end >= now

  return (
    <Card>
      <CardHeader
        title={end === null && hours === 24 ? t("Timeline aujourd'hui", "Timeline today") : 'Timeline'}
        subtitle={`${formatDate(win.start)} ${formatTime(win.start)} → ${formatTime(win.end)}`}
        icon={<Clock className="size-4" />}
        actions={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <Segmented label={t('Durée affichée', 'Shown length')} options={WINDOW_OPTIONS} value={hours} onChange={(h) => setHours(h)} />
            <IconButton label={t('Période précédente', 'Previous period')} icon={<ChevronLeft className="size-4" />} onClick={() => shift(-1)} />
            <IconButton
              label={t('Période suivante', 'Next period')}
              icon={<ChevronRight className="size-4" />}
              onClick={() => shift(1)}
              disabledReason={atPresent ? t('Déjà sur la période actuelle.', 'Already on the current period.') : null}
            />
            <Button size="sm" onClick={() => setEnd(null)} disabledReason={end === null ? t('Déjà sur la période actuelle.', 'Already on the current period.') : null}>
              {t('Maintenant', 'Now')}
            </Button>
          </div>
        }
      />
      <div className="overflow-x-auto px-4 py-4">
        <div className="min-w-[520px]">
          {data.error ? (
            <ErrorState error={data.error} onRetry={data.reload} compact />
          ) : !data.data ? (
            <LoadingState compact label={t('Chargement des enregistrements…', 'Loading recordings…')} />
          ) : (
            <Timeline
              rows={rows}
              start={win.start}
              end={win.end}
              now={now}
              recordings={data.data.periods}
              motion={data.data.motion}
              events={data.data.markers}
              onSelect={onSelect}
            />
          )}
        </div>
      </div>
    </Card>
  )
}
